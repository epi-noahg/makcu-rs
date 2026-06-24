//! Graphical test bench for the MAKCU KM protocol.
//!
//! Drives the `makcu-rs` serial library: pick a port, connect, then fire mouse /
//! keyboard / lock / system commands and watch the device's button stream and
//! every TX/RX line in a live console. A raw-command box lets you exercise any
//! protocol command that has no dedicated button.
//!
//! Run the window:           `cargo run -p makcu-gui` (or from this dir: `cargo run`)
//! List ports (no display):  `cargo run -p makcu-gui -- --list-ports`

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Color32, ComboBox, DragValue, Grid, RichText, ScrollArea, TextEdit};
use makcu_rs::{Device, MakcuResult, MouseButton, MouseButtonStates};
use parking_lot::Mutex;

const MAX_LOG: usize = 1000;

#[derive(Clone, Copy, PartialEq)]
enum Tag {
    Tx,
    Rx,
    Info,
    Err,
}

type LogBuf = Arc<Mutex<VecDeque<(Tag, String)>>>;

fn push_log(log: &LogBuf, tag: Tag, msg: impl Into<String>) {
    let mut l = log.lock();
    if l.len() >= MAX_LOG {
        l.pop_front();
    }
    l.push_back((tag, msg.into()));
}

struct App {
    // connection
    ports: Vec<String>,
    selected_port: Option<String>,
    baud: u32,
    device: Option<Device>,

    // shared, written from the serial reader thread + worker threads
    log: LogBuf,
    buttons: Arc<Mutex<MouseButtonStates>>,

    // mouse move
    dx: i32,
    dy: i32,
    step: i32,
    abs_x: i32,
    abs_y: i32,

    // wheel / pan / tilt
    wheel: i32,
    pan: i32,
    tilt: i32,

    // click scheduling / turbo
    click_btn: i32,
    click_count: i32,
    click_delay: i32,

    // keyboard
    type_text: String,
    key_str: String,

    // locks (local UI state; toggling sends the command)
    lk_mx: bool,
    lk_my: bool,
    lk_mw: bool,
    lk_l: bool,
    lk_r: bool,
    lk_m: bool,
    lk_s1: bool,
    lk_s2: bool,

    // raw console
    raw_cmd: String,
}

impl Default for App {
    fn default() -> Self {
        let mut app = Self {
            ports: Vec::new(),
            selected_port: None,
            baud: 4_000_000,
            device: None,
            log: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LOG))),
            buttons: Arc::new(Mutex::new(MouseButtonStates::default())),
            dx: 50,
            dy: 0,
            step: 25,
            abs_x: 0,
            abs_y: 0,
            wheel: 0,
            pan: 0,
            tilt: 0,
            click_btn: 1,
            click_count: 3,
            click_delay: 0,
            type_text: String::from("hello"),
            key_str: String::from("'a'"),
            lk_mx: false,
            lk_my: false,
            lk_mw: false,
            lk_l: false,
            lk_r: false,
            lk_m: false,
            lk_s1: false,
            lk_s2: false,
            raw_cmd: String::new(),
        };
        app.refresh_ports();
        push_log(
            &app.log,
            Tag::Info,
            "Pick a port and press Connect. This firmware's link is 4000000 baud (default selected).",
        );
        app
    }
}

impl App {
    fn refresh_ports(&mut self) {
        self.ports = serialport::available_ports()
            .map(|v| v.into_iter().map(|p| p.port_name).collect())
            .unwrap_or_default();

        let keep = self
            .selected_port
            .as_ref()
            .map(|s| self.ports.contains(s))
            .unwrap_or(false);
        if !keep {
            self.selected_port = self
                .ports
                .iter()
                .find(|p| {
                    let u = p.to_lowercase();
                    u.contains("usbmodem")
                        || u.contains("usbserial")
                        || u.starts_with("com")
                        || u.contains("acm")
                })
                .or_else(|| self.ports.first())
                .cloned();
        }
    }

    fn connected(&self) -> bool {
        self.device.is_some()
    }

    /// Fire-and-forget: log the command, run it, log any error.
    fn ff<F: FnOnce(&Device) -> MakcuResult<()>>(&self, desc: &str, f: F) {
        let Some(dev) = &self.device else {
            push_log(&self.log, Tag::Err, "not connected");
            return;
        };
        push_log(&self.log, Tag::Tx, desc.to_string());
        if let Err(e) = f(dev) {
            push_log(&self.log, Tag::Err, format!("{desc}  ->  {e}"));
        }
    }

    /// Send a raw protocol line, fire-and-forget (logged like any command).
    /// Used by the gamepad panel for commands the lib has no wrapper for
    /// (km.btnA/B/X/Y, km.lb/rb) so the wire bytes match the firmware exactly.
    fn raw_ff(&self, cmd: &str) {
        let owned = cmd.to_string();
        self.ff(cmd, move |d| d.send_raw(&owned));
    }

    /// Tracked GET: runs on a worker thread (the call blocks until the device
    /// replies) and pushes the reply to the log when it arrives.
    fn get<F>(&self, desc: &str, f: F)
    where
        F: FnOnce(&Device) -> MakcuResult<String> + Send + 'static,
    {
        let Some(dev) = &self.device else {
            push_log(&self.log, Tag::Err, "not connected");
            return;
        };
        let dev = dev.clone();
        let log = self.log.clone();
        let desc = desc.to_string();
        push_log(&self.log, Tag::Tx, desc.clone());
        std::thread::spawn(move || match f(&dev) {
            Ok(s) => push_log(&log, Tag::Rx, format!("{desc}  =>  {}", s.trim())),
            Err(e) => push_log(&log, Tag::Err, format!("{desc}  ->  {e}")),
        });
    }

    fn connect(&mut self) {
        let Some(port) = self.selected_port.clone() else {
            push_log(&self.log, Tag::Err, "no port selected");
            return;
        };
        let dev = Device::new(port.clone(), self.baud, Duration::from_millis(100));
        match dev.connect() {
            Ok(()) => {
                // Stream lines (keyboard / axis / text frames + GET replies w/o id).
                let log = self.log.clone();
                dev.set_stream_callback(Some(move |line: String| {
                    let t = line.trim();
                    if t.is_empty() || t == ">>>" {
                        return;
                    }
                    push_log(&log, Tag::Rx, t.to_string());
                }));
                // Button mask stream (connect() already enabled km.buttons(1,10)).
                let btns = self.buttons.clone();
                dev.set_button_callback(Some(move |st: MouseButtonStates| *btns.lock() = st));

                push_log(
                    &self.log,
                    Tag::Info,
                    format!("connected to {port} @ {}", self.baud),
                );
                self.device = Some(dev);
                self.get("km.version()", |d| d.version());
                push_log(
                    &self.log,
                    Tag::Info,
                    "Note: gamepad-passthrough fw replies ONLY to km.version(). \
                     move/click/buttons inject silently into the target controller \
                     (visible only on the target PC, and only when a real controller \
                     is bridged via the Right MCU). Other commands are accepted but ignored.",
                );
            }
            Err(e) => push_log(&self.log, Tag::Err, format!("connect {port}: {e}")),
        }
    }

    fn disconnect(&mut self) {
        if let Some(dev) = self.device.take() {
            dev.disconnect();
            push_log(&self.log, Tag::Info, "disconnected");
        }
        *self.buttons.lock() = MouseButtonStates::default();
    }

    fn nudge(&self, dx: i32, dy: i32) {
        self.ff(&format!("km.move({dx},{dy})"), move |d| d.move_rel(dx, dy));
    }

    // ---- UI sections -------------------------------------------------------

    fn connection_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("MAKCU");
            ui.separator();
            let online = self.connected();

            ui.add_enabled_ui(!online, |ui| {
                ui.label("Port");
                let current = self.selected_port.clone().unwrap_or_else(|| "—".into());
                ComboBox::from_id_salt("port_combo")
                    .selected_text(current)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for p in &self.ports {
                            ui.selectable_value(&mut self.selected_port, Some(p.clone()), p);
                        }
                    });
                if ui.button("⟳").on_hover_text("Rescan serial ports").clicked() {
                    self.refresh_ports();
                }
                ui.label("Baud");
                ComboBox::from_id_salt("baud_combo")
                    .selected_text(self.baud.to_string())
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for b in [115_200u32, 1_000_000, 2_000_000, 4_000_000] {
                            ui.selectable_value(&mut self.baud, b, b.to_string());
                        }
                    });
            });

            if online {
                if ui.button("Disconnect").clicked() {
                    self.disconnect();
                }
                ui.colored_label(Color32::from_rgb(70, 210, 110), "● online");
            } else if ui.button("Connect").clicked() {
                self.connect();
            }
        });
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let online = self.connected();
        ui.add_enabled_ui(online, |ui| {
            egui::CollapsingHeader::new("🎮 Gamepad — flashed firmware (live device)")
                .default_open(true)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "Exactly what the flashed gamepad-passthrough fw supports. \
                             Aim = right stick; buttons = controller buttons. Effects appear \
                             on the TARGET PC only (and need a real controller bridged via the \
                             Right MCU). No serial reply is expected for these.",
                        )
                        .small()
                        .italics(),
                    );
                    ui.horizontal(|ui| {
                        ui.label("aim Δ");
                        ui.add(DragValue::new(&mut self.dx).speed(1.0));
                        ui.add(DragValue::new(&mut self.dy).speed(1.0));
                        if ui.button("aim (km.move)").clicked() {
                            let (dx, dy) = (self.dx, self.dy);
                            self.ff(&format!("km.move({dx},{dy})"), move |d| d.move_rel(dx, dy));
                        }
                    });
                    Grid::new("gp_btns").spacing([6.0, 4.0]).show(ui, |ui| {
                        ui.label(RichText::new("button").strong());
                        ui.label(RichText::new("hold ↓ / ↑").strong());
                        ui.label("");
                        ui.label(RichText::new("tap").strong());
                        ui.end_row();
                        // Triggers: tap maps to the firmware's km.click(idx) pulse.
                        for (name, base, idx) in [
                            ("Fire / RT", "km.left", 0),
                            ("ADS / LT", "km.right", 1),
                            ("X / mid", "km.middle", 2),
                        ] {
                            ui.label(name);
                            if ui.button("down").clicked() {
                                self.raw_ff(&format!("{base}(1)"));
                            }
                            if ui.button("up").clicked() {
                                self.raw_ff(&format!("{base}(0)"));
                            }
                            if ui.button("tap").clicked() {
                                self.raw_ff(&format!("km.click({idx})"));
                            }
                            ui.end_row();
                        }
                        // Face + bumpers: no km.click form, tap = down then up.
                        for (name, base) in
                            [("A", "km.btnA"), ("B", "km.btnB"), ("Y", "km.btnY"), ("LB", "km.lb"), ("RB", "km.rb")]
                        {
                            ui.label(name);
                            if ui.button("down").clicked() {
                                self.raw_ff(&format!("{base}(1)"));
                            }
                            if ui.button("up").clicked() {
                                self.raw_ff(&format!("{base}(0)"));
                            }
                            if ui.button("tap").clicked() {
                                self.raw_ff(&format!("{base}(1)"));
                                self.raw_ff(&format!("{base}(0)"));
                            }
                            ui.end_row();
                        }
                    });
                });

            egui::CollapsingHeader::new("Mouse — move")
                .default_open(true)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("dx");
                        ui.add(DragValue::new(&mut self.dx).speed(1.0));
                        ui.label("dy");
                        ui.add(DragValue::new(&mut self.dy).speed(1.0));
                        if ui.button("move").clicked() {
                            let (dx, dy) = (self.dx, self.dy);
                            self.ff(&format!("km.move({dx},{dy})"), move |d| d.move_rel(dx, dy));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("step");
                        ui.add(DragValue::new(&mut self.step).speed(1.0));
                    });
                    Grid::new("nudge_pad").spacing([4.0, 4.0]).show(ui, |ui| {
                        ui.label("");
                        if ui.button("  ↑  ").clicked() {
                            self.nudge(0, -self.step);
                        }
                        ui.label("");
                        ui.end_row();
                        if ui.button("  ←  ").clicked() {
                            self.nudge(-self.step, 0);
                        }
                        ui.label("  •  ");
                        if ui.button("  →  ").clicked() {
                            self.nudge(self.step, 0);
                        }
                        ui.end_row();
                        ui.label("");
                        if ui.button("  ↓  ").clicked() {
                            self.nudge(0, self.step);
                        }
                        ui.label("");
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        ui.label("x");
                        ui.add(DragValue::new(&mut self.abs_x).speed(1.0));
                        ui.label("y");
                        ui.add(DragValue::new(&mut self.abs_y).speed(1.0));
                        if ui.button("moveto").clicked() {
                            let (x, y) = (self.abs_x, self.abs_y);
                            self.ff(&format!("km.moveto({x},{y})"), move |d| d.move_abs(x, y));
                        }
                        if ui.button("getpos").clicked() {
                            self.get("km.getpos()", |d| d.get_pos());
                        }
                    });
                });

            egui::CollapsingHeader::new("Mouse — buttons")
                .default_open(true)
                .show(ui, |ui| {
                    for (name, mb) in [
                        ("Left", MouseButton::Left),
                        ("Right", MouseButton::Right),
                        ("Middle", MouseButton::Middle),
                        ("Side1", MouseButton::Side1),
                        ("Side2", MouseButton::Side2),
                    ] {
                        ui.horizontal(|ui| {
                            ui.add_sized([56.0, 18.0], egui::Label::new(name));
                            if ui.button("press").clicked() {
                                self.ff(&format!("press {name}"), move |d| d.press(mb));
                            }
                            if ui.button("release").clicked() {
                                self.ff(&format!("release {name}"), move |d| d.release(mb));
                            }
                            if ui.button("click").clicked() {
                                self.ff(&format!("click {name}"), move |d| d.click(mb));
                            }
                        });
                    }
                });

            egui::CollapsingHeader::new("Wheel / Pan / Tilt").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("wheel");
                    ui.add(DragValue::new(&mut self.wheel).speed(1.0));
                    if ui.button("scroll").clicked() {
                        let w = self.wheel;
                        self.ff(&format!("km.wheel({w})"), move |d| d.wheel(w));
                    }
                    if ui.button("▲").clicked() {
                        self.ff("km.wheel(1)", |d| d.wheel(1));
                    }
                    if ui.button("▼").clicked() {
                        self.ff("km.wheel(-1)", |d| d.wheel(-1));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("pan ");
                    ui.add(DragValue::new(&mut self.pan).speed(1.0));
                    if ui.button("pan").clicked() {
                        let p = self.pan;
                        self.ff(&format!("km.pan({p})"), move |d| d.pan(p));
                    }
                    ui.label("tilt");
                    ui.add(DragValue::new(&mut self.tilt).speed(1.0));
                    if ui.button("tilt").clicked() {
                        let t = self.tilt;
                        self.ff(&format!("km.tilt({t})"), move |d| d.tilt(t));
                    }
                });
            });

            egui::CollapsingHeader::new("Click & turbo").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("button");
                    ui.add(DragValue::new(&mut self.click_btn).range(1..=5).speed(0.1));
                    ui.label("count");
                    ui.add(DragValue::new(&mut self.click_count).range(1..=100).speed(0.2));
                    ui.label("delay ms");
                    ui.add(DragValue::new(&mut self.click_delay).range(0..=5000).speed(1.0));
                });
                ui.horizontal(|ui| {
                    if ui.button("click(n)").clicked() {
                        let b = self.click_btn as u8;
                        let c = self.click_count.max(1) as u32;
                        let dms = (self.click_delay > 0).then_some(self.click_delay as u32);
                        let desc = match dms {
                            Some(d) => format!("km.click({b},{c},{d})"),
                            None => format!("km.click({b},{c})"),
                        };
                        self.ff(&desc, move |d| d.click_scheduled(b, Some(c), dms));
                    }
                    if ui.button("turbo on").clicked() {
                        let b = self.click_btn as u8;
                        let dms = (self.click_delay > 0).then_some(self.click_delay as u32);
                        self.ff(&format!("km.turbo({b})"), move |d| d.turbo(b, dms));
                    }
                    if ui.button("turbo off").clicked() {
                        self.ff("km.turbo(0)", |d| d.turbo_disable_all());
                    }
                    if ui.button("turbo?").clicked() {
                        self.get("km.turbo()", |d| d.turbo_get());
                    }
                });
            });

            egui::CollapsingHeader::new("Keyboard").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("text");
                    ui.add(TextEdit::singleline(&mut self.type_text).desired_width(150.0));
                    if ui.button("type").clicked() {
                        let t = self.type_text.clone();
                        self.ff(&format!("km.string(\"{t}\")"), move |d| d.type_string(&t));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("key");
                    ui.add(
                        TextEdit::singleline(&mut self.key_str)
                            .desired_width(70.0)
                            .hint_text("'a' or 4"),
                    );
                    if ui.button("down").clicked() {
                        let k = self.key_str.clone();
                        self.ff(&format!("km.down({k})"), move |d| d.key_down(&k));
                    }
                    if ui.button("up").clicked() {
                        let k = self.key_str.clone();
                        self.ff(&format!("km.up({k})"), move |d| d.key_up(&k));
                    }
                    if ui.button("press").clicked() {
                        let k = self.key_str.clone();
                        self.ff(&format!("km.press({k})"), move |d| d.key_press(&k, None, None));
                    }
                });
                if ui.button("init (release all)").clicked() {
                    self.ff("km.init()", |d| d.keyboard_init());
                }
            });

            egui::CollapsingHeader::new("Locks (physical)").show(ui, |ui| {
                ui.label("Axes");
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.lk_mx, "X").changed() {
                        let v = self.lk_mx;
                        self.ff(&format!("km.lock_mx({})", v as u8), move |d| d.lock_mouse_x(v));
                    }
                    if ui.checkbox(&mut self.lk_my, "Y").changed() {
                        let v = self.lk_my;
                        self.ff(&format!("km.lock_my({})", v as u8), move |d| d.lock_mouse_y(v));
                    }
                    if ui.checkbox(&mut self.lk_mw, "Wheel").changed() {
                        let v = self.lk_mw;
                        self.ff(&format!("km.lock_mw({})", v as u8), move |d| {
                            d.lock_mouse_wheel(v)
                        });
                    }
                });
                ui.label("Buttons");
                ui.horizontal_wrapped(|ui| {
                    if ui.checkbox(&mut self.lk_l, "L").changed() {
                        let v = self.lk_l;
                        self.ff(&format!("km.lock_ml({})", v as u8), move |d| d.lock_left(v));
                    }
                    if ui.checkbox(&mut self.lk_r, "R").changed() {
                        let v = self.lk_r;
                        self.ff(&format!("km.lock_mr({})", v as u8), move |d| d.lock_right(v));
                    }
                    if ui.checkbox(&mut self.lk_m, "M").changed() {
                        let v = self.lk_m;
                        self.ff(&format!("km.lock_mm({})", v as u8), move |d| d.lock_middle(v));
                    }
                    if ui.checkbox(&mut self.lk_s1, "S1").changed() {
                        let v = self.lk_s1;
                        self.ff(&format!("km.lock_ms1({})", v as u8), move |d| d.lock_side1(v));
                    }
                    if ui.checkbox(&mut self.lk_s2, "S2").changed() {
                        let v = self.lk_s2;
                        self.ff(&format!("km.lock_ms2({})", v as u8), move |d| d.lock_side2(v));
                    }
                });
            });

            egui::CollapsingHeader::new("System / streaming").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("version").clicked() {
                        self.get("km.version()", |d| d.version());
                    }
                    if ui.button("info").clicked() {
                        self.get("km.info()", |d| d.info());
                    }
                    if ui.button("device").clicked() {
                        self.get("km.device()", |d| d.device_type());
                    }
                    if ui.button("fault").clicked() {
                        self.get("km.fault()", |d| d.fault());
                    }
                    if ui.button("reboot").clicked() {
                        self.ff("km.reboot()", |d| d.reboot());
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("kbd stream on").clicked() {
                        self.ff("km.keyboard(1,1)", |d| d.keyboard_stream(1, Some(1)));
                    }
                    if ui.button("kbd stream off").clicked() {
                        self.ff("km.keyboard(0)", |d| d.keyboard_stream(0, None));
                    }
                    if ui.button("axis stream on").clicked() {
                        self.ff("km.axis(1,10)", |d| d.axis_stream(1, Some(10)));
                    }
                    if ui.button("axis stream off").clicked() {
                        self.ff("km.axis(0)", |d| d.axis_stream(0, None));
                    }
                });
            });
        });
    }

    fn monitor(&mut self, ui: &mut egui::Ui) {
        ui.heading("Live monitor");
        let bs = *self.buttons.lock();
        ui.horizontal(|ui| {
            ui.label("Buttons:");
            led(ui, "L", bs.left);
            led(ui, "R", bs.right);
            led(ui, "M", bs.middle);
            led(ui, "S1", bs.side1);
            led(ui, "S2", bs.side2);
        });
        ui.label(
            RichText::new("(LEDs update only on mouse-profile fw; the gamepad fw doesn't stream km.buttons)")
                .small()
                .weak(),
        );
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(format!("Console ({} lines)", self.log.lock().len()));
            if ui.button("clear").clicked() {
                self.log.lock().clear();
            }
        });
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let log = self.log.lock();
                for (tag, msg) in log.iter() {
                    let (col, pfx) = match tag {
                        Tag::Tx => (Color32::from_rgb(120, 170, 255), "TX"),
                        Tag::Rx => (Color32::from_rgb(120, 230, 140), "RX"),
                        Tag::Info => (Color32::GRAY, "··"),
                        Tag::Err => (Color32::from_rgb(240, 120, 120), "!!"),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(col, RichText::new(pfx).monospace());
                        ui.label(RichText::new(msg).monospace());
                    });
                }
            });
    }

    fn console_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("raw");
            let avail = (ui.available_width() - 210.0).max(120.0);
            let resp = ui.add(
                TextEdit::singleline(&mut self.raw_cmd)
                    .desired_width(avail)
                    .hint_text("km.move(10,0)   ·   .version()   ·   km.mo(1,0,0,0,0,0)"),
            );
            let enter =
                resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            let mut send = false;
            let mut track = false;
            if ui.button("Send").clicked() {
                send = true;
            }
            if ui.button("Send + wait").clicked() {
                track = true;
            }
            if enter {
                send = true;
            }

            if send || track {
                let c = self.raw_cmd.trim().to_string();
                if !c.is_empty() {
                    if track {
                        let cc = c.clone();
                        self.get(&c, move |d| d.send_raw_tracked(&cc, 2.0));
                    } else {
                        let cc = c.clone();
                        self.ff(&c, move |d| d.send_raw(&cc));
                    }
                    self.raw_cmd.clear();
                    resp.request_focus();
                }
            }
        });
    }
}

fn led(ui: &mut egui::Ui, name: &str, on: bool) {
    let col = if on {
        Color32::from_rgb(70, 220, 100)
    } else {
        Color32::from_gray(80)
    };
    ui.colored_label(col, format!("● {name}"));
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("connection").show_inside(ui, |ui| {
            ui.add_space(2.0);
            self.connection_bar(ui);
            ui.add_space(2.0);
        });

        egui::Panel::bottom("console_input").show_inside(ui, |ui| {
            ui.add_space(2.0);
            self.console_bar(ui);
            ui.add_space(2.0);
        });

        egui::Panel::left("controls")
            .resizable(true)
            .default_size(350.0)
            .show_inside(ui, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.controls(ui));
            });

        egui::CentralPanel::default().show_inside(ui, |ui| self.monitor(ui));

        // Keep refreshing so reader-thread / worker output appears without input.
        ui.ctx().request_repaint_after(Duration::from_millis(80));
    }
}

fn list_ports() {
    match serialport::available_ports() {
        Ok(ports) if ports.is_empty() => println!("(no serial ports found)"),
        Ok(ports) => {
            for p in ports {
                println!("{:<28} {:?}", p.port_name, p.port_type);
            }
        }
        Err(e) => eprintln!("error listing ports: {e}"),
    }
}

/// Auto-pick the most likely MAKCU port (same heuristic the UI uses).
fn autodetect_port() -> Option<String> {
    let ports: Vec<String> = serialport::available_ports()
        .map(|v| v.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default();
    ports
        .iter()
        .find(|p| {
            let u = p.to_lowercase();
            u.contains("usbmodem") || u.contains("usbserial") || u.contains("acm")
        })
        .or_else(|| ports.first())
        .cloned()
}

/// Headless connectivity check: connect, query the firmware, print replies.
/// Read-only — never sends movement/clicks, safe against a live device.
///
/// `km.version()` is the one reply this firmware guarantees (other commands are
/// accepted but answer silently when diag/echo is off), so the exit code keys on
/// it: 0 = firmware answered version(), 1 = no answer, 2 = no port.
fn selftest(port: Option<String>, baud: u32) -> i32 {
    let Some(port) = port.or_else(autodetect_port) else {
        eprintln!("selftest: no serial port found");
        return 2;
    };
    println!("selftest: connecting to {port} @ {baud} ...");
    let dev = Device::new(port.clone(), baud, Duration::from_millis(200));
    if let Err(e) = dev.connect() {
        eprintln!("selftest: connect failed: {e}");
        return 1;
    }

    // version() is the authoritative connectivity proof.
    let alive = match dev.version() {
        Ok(reply) => {
            println!("  ok   km.version()  -> {}", reply.trim());
            true
        }
        Err(e) => {
            println!("  FAIL km.version()  -> {e}");
            false
        }
    };

    // Best-effort extras: a timeout here is normal (silent unless diag is on).
    let extras: [(&str, fn(&Device) -> MakcuResult<String>); 2] =
        [("km.device()", |d| d.device_type()), ("km.info()", |d| d.info())];
    for (name, f) in extras {
        match f(&dev) {
            Ok(reply) => println!("  ok   {name:<13} -> {}", reply.trim()),
            Err(_) => println!("  --   {name:<13} -> (no reply; expected when diag off)"),
        }
    }

    dev.disconnect();
    if alive {
        println!("selftest: PASS — firmware responded to km.version()");
        0
    } else {
        println!("selftest: FAIL — no version() reply (check baud; this firmware uses 4000000)");
        1
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--list-ports") {
        list_ports();
        return Ok(());
    }

    // shared: --baud N (defaults to this firmware's 4 Mbaud link)
    let baud = args
        .iter()
        .position(|a| a == "--baud")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(4_000_000);

    if let Some(i) = args.iter().position(|a| a == "--selftest") {
        // optional positional port right after the flag, e.g. `--selftest /dev/cu.usbmodemXXXX`
        let port = args.get(i + 1).filter(|a| !a.starts_with("--")).cloned();
        std::process::exit(selftest(port, baud));
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 660.0])
            .with_min_inner_size([720.0, 460.0])
            .with_title("MAKCU protocol tester"),
        ..Default::default()
    };

    eframe::run_native(
        "MAKCU protocol tester",
        native_options,
        Box::new(|_cc| Ok(Box::new(App::default()))),
    )
}

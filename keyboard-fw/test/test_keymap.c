// Phase 1 — tests for core/keymap.
#include "test_framework.h"
#include "keymap.h"

// ---- key_lookup: numeric HID codes (unquoted) --------------------------------
static void test_numeric_code(void) {
    uint8_t code; bool shift;
    ASSERT_TRUE(key_lookup("4", &code, &shift));
    ASSERT_EQ_INT(4, code);
    ASSERT_FALSE(shift);

    ASSERT_TRUE(key_lookup("224", &code, &shift));
    ASSERT_EQ_INT(224, code);

    ASSERT_FALSE(key_lookup("256", &code, &shift)); // out of u8 range
}

// ---- key_lookup: single letters are case-sensitive ---------------------------
static void test_letters(void) {
    uint8_t code; bool shift;
    ASSERT_TRUE(key_lookup("'a'", &code, &shift));
    ASSERT_EQ_INT(4, code); ASSERT_FALSE(shift);

    ASSERT_TRUE(key_lookup("'A'", &code, &shift));
    ASSERT_EQ_INT(4, code); ASSERT_TRUE(shift);   // uppercase => auto-shift

    ASSERT_TRUE(key_lookup("'z'", &code, &shift));
    ASSERT_EQ_INT(29, code); ASSERT_FALSE(shift);

    // unquoted single letter accepted too
    ASSERT_TRUE(key_lookup("b", &code, &shift));
    ASSERT_EQ_INT(5, code);
}

// ---- key_lookup: digit *keys* via quoted names -------------------------------
static void test_digit_keys(void) {
    uint8_t code; bool shift;
    ASSERT_TRUE(key_lookup("'1'", &code, &shift));
    ASSERT_EQ_INT(30, code); ASSERT_FALSE(shift);

    ASSERT_TRUE(key_lookup("'0'", &code, &shift));
    ASSERT_EQ_INT(39, code);
}

// ---- key_lookup: named specials are case-insensitive -------------------------
static void test_named(void) {
    uint8_t code; bool shift;
    ASSERT_TRUE(key_lookup("'enter'", &code, &shift));
    ASSERT_EQ_INT(40, code);
    ASSERT_TRUE(key_lookup("\"ENTER\"", &code, &shift)); // double quotes + caps
    ASSERT_EQ_INT(40, code);

    ASSERT_TRUE(key_lookup("'f1'", &code, &shift));
    ASSERT_EQ_INT(58, code);
    ASSERT_TRUE(key_lookup("'F1'", &code, &shift));
    ASSERT_EQ_INT(58, code);
    ASSERT_TRUE(key_lookup("'f12'", &code, &shift));
    ASSERT_EQ_INT(69, code);

    ASSERT_TRUE(key_lookup("'space'", &code, &shift));
    ASSERT_EQ_INT(44, code);
}

// ---- key_lookup: modifiers default to the left variant -----------------------
static void test_modifiers(void) {
    uint8_t code; bool shift;
    ASSERT_TRUE(key_lookup("'ctrl'", &code, &shift));
    ASSERT_EQ_INT(224, code);
    ASSERT_TRUE(key_lookup("'shift'", &code, &shift));
    ASSERT_EQ_INT(225, code);
    ASSERT_TRUE(key_lookup("'alt'", &code, &shift));
    ASSERT_EQ_INT(226, code);
    ASSERT_TRUE(key_lookup("'gui'", &code, &shift));
    ASSERT_EQ_INT(227, code);
    ASSERT_TRUE(key_lookup("'win'", &code, &shift));
    ASSERT_EQ_INT(227, code);
    ASSERT_TRUE(key_lookup("'rctrl'", &code, &shift));
    ASSERT_EQ_INT(228, code);

    ASSERT_TRUE(key_is_modifier(224));
    ASSERT_TRUE(key_is_modifier(231));
    ASSERT_FALSE(key_is_modifier(4));
}

// ---- key_lookup: unknown -----------------------------------------------------
static void test_unknown(void) {
    uint8_t code; bool shift;
    ASSERT_FALSE(key_lookup("'nope'", &code, &shift));
    ASSERT_FALSE(key_lookup("", &code, &shift));
    ASSERT_FALSE(key_lookup("''", &code, &shift));
}

// ---- ascii_to_hid: for km.string typing --------------------------------------
static void test_ascii_to_hid(void) {
    uint8_t code; bool shift;
    ASSERT_TRUE(ascii_to_hid('a', &code, &shift));
    ASSERT_EQ_INT(4, code); ASSERT_FALSE(shift);

    ASSERT_TRUE(ascii_to_hid('A', &code, &shift));
    ASSERT_EQ_INT(4, code); ASSERT_TRUE(shift);

    ASSERT_TRUE(ascii_to_hid('1', &code, &shift));
    ASSERT_EQ_INT(30, code); ASSERT_FALSE(shift);

    ASSERT_TRUE(ascii_to_hid('!', &code, &shift));
    ASSERT_EQ_INT(30, code); ASSERT_TRUE(shift);

    ASSERT_TRUE(ascii_to_hid(' ', &code, &shift));
    ASSERT_EQ_INT(44, code); ASSERT_FALSE(shift);

    ASSERT_TRUE(ascii_to_hid('\n', &code, &shift));
    ASSERT_EQ_INT(40, code);

    ASSERT_TRUE(ascii_to_hid('\t', &code, &shift));
    ASSERT_EQ_INT(43, code);

    ASSERT_TRUE(ascii_to_hid('?', &code, &shift));
    ASSERT_EQ_INT(56, code); ASSERT_TRUE(shift);

    ASSERT_TRUE(ascii_to_hid('-', &code, &shift));
    ASSERT_EQ_INT(45, code); ASSERT_FALSE(shift);

    ASSERT_TRUE(ascii_to_hid('_', &code, &shift));
    ASSERT_EQ_INT(45, code); ASSERT_TRUE(shift);
}

int main(void) {
    TF_BEGIN();
    TF_RUN(test_numeric_code);
    TF_RUN(test_letters);
    TF_RUN(test_digit_keys);
    TF_RUN(test_named);
    TF_RUN(test_modifiers);
    TF_RUN(test_unknown);
    TF_RUN(test_ascii_to_hid);
    return TF_END();
}

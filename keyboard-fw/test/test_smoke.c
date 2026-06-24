// Phase 0 smoke test — proves the harness compiles, links and runs.
#include "test_framework.h"

static void test_harness_runs(void) {
    ASSERT_TRUE(1);
    ASSERT_EQ_INT(2, 1 + 1);
    ASSERT_EQ_STR("ok", "ok");
}

int main(void) {
    TF_BEGIN();
    TF_RUN(test_harness_runs);
    return TF_END();
}

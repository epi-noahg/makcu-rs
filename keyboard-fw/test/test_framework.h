// test_framework.h — minimal, dependency-free C assertion harness.
//
// Each test file is its own executable:
//
//   #include "test_framework.h"
//   static void test_foo(void) { ASSERT_EQ_INT(2, 1 + 1); }
//   int main(void) { TF_BEGIN(); TF_RUN(test_foo); return TF_END(); }
//
// Runs natively with any C11 compiler (cc/gcc/clang). A PlatformIO `native`
// (Unity) env is also provided for CI parity, but `make test` is the canonical
// local runner since it needs nothing but a compiler.
#ifndef TEST_FRAMEWORK_H
#define TEST_FRAMEWORK_H

#include <stdio.h>
#include <string.h>

static int tf_tests;
static int tf_failed;
static int tf_cur_fail;

#define TF_BEGIN() do { tf_tests = 0; tf_failed = 0; tf_cur_fail = 0; } while (0)

#define TF_RUN(fn)                                                            \
    do {                                                                      \
        tf_cur_fail = 0;                                                       \
        fn();                                                                 \
        tf_tests++;                                                           \
        if (tf_cur_fail) { tf_failed++; printf("  [FAIL] %s\n", #fn); }        \
        else             { printf("  [ ok ] %s\n", #fn); }                    \
    } while (0)

#define TF_END()                                                              \
    (printf("\n%d tests, %d failed\n", tf_tests, tf_failed), tf_failed ? 1 : 0)

#define TF_FAILF(...)                                                         \
    do {                                                                      \
        tf_cur_fail = 1;                                                      \
        printf("    %s:%d: ", __FILE__, __LINE__);                            \
        printf(__VA_ARGS__);                                                  \
        printf("\n");                                                         \
    } while (0)

#define ASSERT_TRUE(cond)                                                     \
    do { if (!(cond)) TF_FAILF("ASSERT_TRUE(%s)", #cond); } while (0)

#define ASSERT_FALSE(cond)                                                    \
    do { if ((cond)) TF_FAILF("ASSERT_FALSE(%s)", #cond); } while (0)

#define ASSERT_EQ_INT(exp, act)                                               \
    do {                                                                      \
        long _e = (long)(exp), _a = (long)(act);                              \
        if (_e != _a)                                                         \
            TF_FAILF("ASSERT_EQ_INT(%s): expected %ld, got %ld", #act, _e, _a); \
    } while (0)

#define ASSERT_EQ_STR(exp, act)                                               \
    do {                                                                      \
        const char *_e = (exp), *_a = (act);                                  \
        if (strcmp(_e, _a) != 0)                                              \
            TF_FAILF("ASSERT_EQ_STR(%s): expected \"%s\", got \"%s\"",        \
                     #act, _e, _a);                                           \
    } while (0)

#define ASSERT_MEM_EQ(exp, act, n)                                            \
    do {                                                                      \
        if (memcmp((exp), (act), (n)) != 0)                                   \
            TF_FAILF("ASSERT_MEM_EQ(%s) over %d bytes", #act, (int)(n));       \
    } while (0)

#endif // TEST_FRAMEWORK_H

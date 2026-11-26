package dev.frust.authsession

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * JVM-only coverage for [authTabResultKind] — how an Auth Tab result code
 * becomes a `nativeOnAuthSessionResult` kind. The codes are written as the
 * literal values `androidx.browser.auth.AuthTabIntent` documents, so a
 * library that renumbered them would fail here rather than on a phone. The
 * kinds are the `KIND_*` constants, which `plugins/auth-session/src/android.rs`
 * mirrors as its `RESULT_*` table.
 */
class AuthTabResultTest {

    @Test
    fun `RESULT_OK is a callback`() {
        assertEquals(KIND_CALLBACK, authTabResultKind(-1))
    }

    @Test
    fun `RESULT_CANCELED is a cancel`() {
        assertEquals(KIND_CANCELLED, authTabResultKind(0))
    }

    @Test
    fun `verification failure and timeout are distinct errors`() {
        assertEquals(KIND_AUTH_TAB_VERIFICATION_FAILED, authTabResultKind(2))
        assertEquals(KIND_AUTH_TAB_VERIFICATION_TIMED_OUT, authTabResultKind(3))
    }

    @Test
    fun `RESULT_UNKNOWN_CODE and any unlisted code are the unknown-result error`() {
        for (code in listOf(-2, 1, 4, 99, Int.MIN_VALUE)) {
            assertEquals("code $code", KIND_AUTH_TAB_UNKNOWN_RESULT, authTabResultKind(code))
        }
    }

    @Test
    fun `the kind numbers match the Rust table`() {
        assertEquals(
            listOf(0, 1, 2, 3, 4, 5, 6, 7, 8),
            listOf(
                KIND_CALLBACK,
                KIND_CANCELLED,
                KIND_NO_BROWSER,
                KIND_LAUNCH_FAILED,
                KIND_NO_RESUMED_ACTIVITY,
                KIND_HTTPS_UNSUPPORTED,
                KIND_AUTH_TAB_VERIFICATION_FAILED,
                KIND_AUTH_TAB_VERIFICATION_TIMED_OUT,
                KIND_AUTH_TAB_UNKNOWN_RESULT,
            ),
        )
    }
}

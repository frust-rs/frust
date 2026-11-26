package dev.frust.authsession

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * JVM-only coverage for [authTabResultKind] — how an Auth Tab result code
 * becomes a `nativeOnAuthSessionResult` kind — for [postedLaunchKind], the
 * mapping from a `start` return code to the kind a posted launch reports,
 * and for [authTabLifecycleAction], what an Activity lifecycle event does to
 * a pending Auth Tab session. The codes are written as the literal values
 * `androidx.browser.auth.AuthTabIntent` documents, so a library that
 * renumbered them would fail here rather than on a phone. The kinds are the
 * `KIND_*` constants and the return codes the `START_*` ones, which
 * `plugins/auth-session/src/android.rs` mirrors as its `RESULT_*` and
 * `START_*` tables.
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

    @Test
    fun `the return-code numbers match the Rust table`() {
        assertEquals(
            listOf(0, 1, 2, 3, 5),
            listOf(START_ACCEPTED, START_NO_RESUMED_ACTIVITY, START_NO_BROWSER, START_LAUNCH_FAILED, START_HTTPS_UNSUPPORTED),
        )
    }

    @Test
    fun `an accepted posted launch reports nothing`() {
        assertNull(postedLaunchKind(START_ACCEPTED))
    }

    @Test
    fun `each posted launch failure maps to its own kind`() {
        assertEquals(KIND_NO_RESUMED_ACTIVITY, postedLaunchKind(START_NO_RESUMED_ACTIVITY))
        assertEquals(KIND_NO_BROWSER, postedLaunchKind(START_NO_BROWSER))
        assertEquals(KIND_LAUNCH_FAILED, postedLaunchKind(START_LAUNCH_FAILED))
        assertEquals(KIND_HTTPS_UNSUPPORTED, postedLaunchKind(START_HTTPS_UNSUPPORTED))
    }

    @Test
    fun `no resumed Activity is never reported as the cancelled kind`() {
        // Return code 1 and kind 1 are different things: the mapping must re-code it.
        assertEquals(1, START_NO_RESUMED_ACTIVITY)
        assertEquals(1, KIND_CANCELLED)
        assertEquals(4, postedLaunchKind(START_NO_RESUMED_ACTIVITY))
    }

    @Test
    fun `an unknown return code is a launch failure, never passed through as a kind`() {
        for (code in listOf(4, 6, 7, 8, -1, 99)) {
            assertEquals("code $code", KIND_LAUNCH_FAILED, postedLaunchKind(code))
        }
    }

    @Test
    fun `nothing happens to a session whose tab has not been launched`() {
        for (event in AuthTabEvent.entries) {
            for (flag in listOf(false, true)) {
                assertEquals(
                    "$event $flag",
                    AuthTabAction.KEEP,
                    authTabLifecycleAction(
                        event,
                        launched = false,
                        hasOwner = flag,
                        isOwner = flag,
                        finishing = !flag,
                        savedState = flag,
                        sameTaskAndClass = flag,
                        keyRestored = flag,
                    ),
                )
            }
        }
    }

    @Test
    fun `leaving the app and coming back never resolves a session whose launcher is attached`() {
        assertEquals(AuthTabAction.KEEP, authTabLifecycleAction(AuthTabEvent.RESUMED, launched = true, hasOwner = true))
        // Another Activity created while the launcher is attached changes nothing either.
        assertEquals(AuthTabAction.KEEP, authTabLifecycleAction(AuthTabEvent.CREATED, launched = true, hasOwner = true))
    }

    @Test
    fun `a destroyed bystander changes nothing`() {
        for (finishing in listOf(false, true)) {
            for (hasOwner in listOf(false, true)) {
                assertEquals(
                    AuthTabAction.KEEP,
                    authTabLifecycleAction(
                        AuthTabEvent.DESTROYED,
                        launched = true,
                        hasOwner = hasOwner,
                        isOwner = false,
                        finishing = finishing,
                    ),
                )
            }
        }
    }

    @Test
    fun `a finishing launcher resolves the session cancelled`() {
        val action = authTabLifecycleAction(
            AuthTabEvent.DESTROYED,
            launched = true,
            hasOwner = true,
            isOwner = true,
            finishing = true,
        )
        assertEquals(AuthTabAction.CANCEL_FINISHED, action)
        assertNotNull(action.cancelReason)
    }

    @Test
    fun `a launcher destroyed to be recreated detaches and waits`() {
        assertEquals(
            AuthTabAction.DETACH,
            authTabLifecycleAction(AuthTabEvent.DESTROYED, launched = true, hasOwner = true, isOwner = true, finishing = false),
        )
    }

    @Test
    fun `a recreated launcher with its state and key re-attaches`() {
        assertEquals(
            AuthTabAction.REATTACH,
            authTabLifecycleAction(
                AuthTabEvent.CREATED,
                launched = true,
                hasOwner = false,
                savedState = true,
                sameTaskAndClass = true,
                keyRestored = true,
            ),
        )
    }

    @Test
    fun `a recreated launcher missing its saved state or its key resolves cancelled`() {
        for ((savedState, keyRestored) in listOf(false to false, false to true, true to false)) {
            assertEquals(
                "savedState=$savedState keyRestored=$keyRestored",
                AuthTabAction.CANCEL_NOT_RESTORED,
                authTabLifecycleAction(
                    AuthTabEvent.CREATED,
                    launched = true,
                    hasOwner = false,
                    savedState = savedState,
                    sameTaskAndClass = true,
                    keyRestored = keyRestored,
                ),
            )
        }
    }

    @Test
    fun `another Activity created while the launcher is gone resolves cancelled`() {
        for (savedState in listOf(false, true)) {
            assertEquals(
                AuthTabAction.CANCEL_OTHER_ACTIVITY,
                authTabLifecycleAction(
                    AuthTabEvent.CREATED,
                    launched = true,
                    hasOwner = false,
                    savedState = savedState,
                    sameTaskAndClass = false,
                    keyRestored = true,
                ),
            )
        }
    }

    @Test
    fun `a resume with no launcher attached resolves cancelled`() {
        assertEquals(
            AuthTabAction.CANCEL_NO_LAUNCHER,
            authTabLifecycleAction(AuthTabEvent.RESUMED, launched = true, hasOwner = false),
        )
    }

    @Test
    fun `exactly the cancel answers carry a reason, each a different one`() {
        val cancels = AuthTabAction.entries.filter { it.name.startsWith("CANCEL_") }
        for (action in AuthTabAction.entries) {
            assertEquals(action.name, action in cancels, action.cancelReason != null)
        }
        assertEquals(cancels.size, cancels.map { it.cancelReason }.toSet().size)
    }
}

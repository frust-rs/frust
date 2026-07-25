package dev.frust.securestorage

import android.content.Context
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicInteger

/**
 * The `frust-secure-storage` biometric helper.
 *
 * The framework `BiometricPrompt.AuthenticationCallback` is an abstract class,
 * and JNI cannot subclass a Java/Kotlin class at runtime, so this tiny helper
 * must exist as bytecode in the installed app.
 *
 * Nothing copies this file anywhere: it ships as part of this plugin's own
 * `com.android.library` module (`plugins/secure-storage/platform/android`),
 * which `frust tui`'s "Add Plugin" dialog wires into a generated project as
 * `:frust-secure-storage` (see the plugin `README.md` for the by-hand
 * equivalent). The module's manifest carries the `USE_BIOMETRIC` permission and
 * its `consumer-rules.pro` carries the R8 keep rule, so a consuming app edits
 * neither of its own.
 *
 * The package/class name `dev.frust.securestorage.FrustBiometric` — and the
 * [authenticate] signature below — are a hard contract: the Rust backend
 * (`plugins/secure-storage/src/android.rs`, `HELPER_CLASS_BINARY`) looks the
 * class up by that exact string through the application classloader. `dev.frust`
 * itself belongs exclusively to the `frust-embedding` module; every plugin takes
 * a subpackage.
 *
 * [authenticate] is called over JNI on a background thread (the plugin's
 * never-on-the-UI-thread invariant). It posts the system prompt to the main
 * executor, subclasses the abstract callback, and blocks the caller on a latch
 * until the prompt resolves. It returns `0` on success or a framework
 * `BiometricPrompt.BIOMETRIC_ERROR_*` code on failure — the Rust side maps that
 * code to a typed error (canceled, lockout, not-enrolled, …).
 */
object FrustBiometric {
    @JvmStatic
    fun authenticate(
        context: Context,
        title: String,
        subtitle: String,
        negativeButton: String,
        allowDeviceCredential: Boolean,
        cryptoObject: BiometricPrompt.CryptoObject,
    ): Int {
        val latch = CountDownLatch(1)
        // Default to CANCELED so a dropped/never-delivered callback fails closed.
        val result = AtomicInteger(BiometricPrompt.BIOMETRIC_ERROR_CANCELED)
        val executor = context.mainExecutor

        executor.execute {
            val builder = BiometricPrompt.Builder(context).setTitle(title)
            if (subtitle.isNotEmpty()) builder.setSubtitle(subtitle)
            // A device-credential fallback and a negative button are mutually
            // exclusive; prefer the fallback when the app allowed it (API 30+),
            // else show the negative (cancel) button.
            if (allowDeviceCredential && Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                builder.setAllowedAuthenticators(
                    BiometricManager.Authenticators.BIOMETRIC_STRONG or
                        BiometricManager.Authenticators.DEVICE_CREDENTIAL,
                )
            } else {
                builder.setNegativeButton(
                    negativeButton.ifEmpty { "Cancel" },
                    executor,
                ) { _, _ -> }
            }
            builder.build().authenticate(
                cryptoObject,
                CancellationSignal(),
                executor,
                object : BiometricPrompt.AuthenticationCallback() {
                    override fun onAuthenticationSucceeded(r: BiometricPrompt.AuthenticationResult) {
                        result.set(0)
                        latch.countDown()
                    }

                    override fun onAuthenticationError(code: Int, msg: CharSequence) {
                        result.set(code)
                        latch.countDown()
                    }
                    // onAuthenticationFailed() is a single non-terminal mismatch;
                    // the framework keeps the prompt up, so it is intentionally
                    // not latched here.
                },
            )
        }

        latch.await()
        return result.get()
    }
}

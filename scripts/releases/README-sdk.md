# Kausway Android client SDK

This SDK shares its release version with the client engine and CLI. Verify payload
checksums and source/build provenance before use. Development/candidate artifacts
are not evidence of a published release; check the release page for publication.

Distribution includes AAR, POM, source JAR and a project/third-party notices archive
covering the ARM64, ARMv7 and x86-64 JNI builds. Retain the applicable notices when
redistributing. Android API 24+ is required; this package does not add iOS support.

Consume the AAR/POM through your own local Maven repository to retain dependency
metadata, or use a file dependency and supply the POM's dependencies explicitly.
For a direct AAR dependency, the current SDK requires
`org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2`. Do not interpret a raw AAR
file dependency as automatic POM resolution. The platform-verifier JAR and supported
native libraries are embedded in the AAR.

The SDK does not start a CLI process or local HTTP proxy automatically. It exposes
the shared client engine through Kotlin/JNI; follow the SDK integration and lifecycle
documentation at https://github.com/Xaque8787/Kausway/tree/main/docs/android.
The CLI client is a separate download, even though it shares this release version.

# Verification

See [validation](validation.md) and the checked-in Rust, process and browser tests.
They cover attributed evidence selection, instruction-only exports, versioned
profile application, revision conflicts, blinded comparisons and human feedback.
Browser fixtures use invented subjects and temporary files. No real person's
profile, source artifacts or extracted guidance is distributed with this project.

Automated checks establish workflow behavior. They do not establish the fidelity
of a profile. Evaluate new profiles with held-out material and explicit feedback
from the intended subject; keep that material in a private installation.

Native CI runs the Rust suite on Linux x64/ARM64, macOS Intel/Apple silicon,
Windows x64 and Windows 11 ARM64. It extracts each produced package and launches
it with system developer tools absent from PATH: local account creation,
identity-verified reopen, private Python, real document import, PDF limitations
and preserved saved evidence across restart. The browser case also loses the
account-creation response after commit and verifies ordinary sign-in recovery.
Windows x64 CI runs on Server 2025; it is not a Windows 11 desktop trust test.
Package import checks and private runtime notices do not establish publisher
signing, notarization, model-provider behavior or profile fidelity.

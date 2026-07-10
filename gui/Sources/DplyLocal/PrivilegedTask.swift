import Foundation

/// Runs one command as root behind macOS's own authorization dialog.
///
/// Privileged fixes used to be handed to Terminal: the user left the app, read a
/// `sudo …` line they hadn't written, and typed their password into a shell. The
/// system authentication sheet is the honest surface for this — it names the app
/// asking, it's the dialog users already know, and the password never passes
/// through us.
///
/// This is deliberately *not* `SMAppService` / `SMJobBless`. Those need a Developer
/// ID-signed bundle to install a persistent privileged helper, and they're the right
/// destination once the app is signed. This runs a single command, once, with no
/// resident helper left behind — which is exactly what `dpl setup` is.
enum PrivilegedTask {
    struct Failure: LocalizedError {
        let message: String
        /// The user dismissed the authentication sheet. Not an error worth shouting about.
        let cancelled: Bool
        var errorDescription: String? { message }
    }

    /// AppleScript's `-128` is "user cancelled".
    private static let userCancelled = -128

    /// Execute `command` as root. Must be called on the main thread: `NSAppleScript`
    /// is not thread-safe, and the authorization sheet is UI.
    @MainActor
    @discardableResult
    static func run(_ command: String) throws -> String {
        let source = "do shell script \"\(escapeForAppleScript(command))\" with administrator privileges"
        guard let script = NSAppleScript(source: source) else {
            throw Failure(message: "Couldn't build the authorization script.", cancelled: false)
        }

        var errorInfo: NSDictionary?
        let result = script.executeAndReturnError(&errorInfo)

        if let errorInfo {
            let code = errorInfo[NSAppleScript.errorNumber] as? Int ?? 0
            if code == userCancelled {
                throw Failure(message: "Authorization cancelled.", cancelled: true)
            }
            let detail = errorInfo[NSAppleScript.errorMessage] as? String ?? "Unknown error"
            throw Failure(message: "Privileged step failed: \(detail)", cancelled: false)
        }
        return result.stringValue ?? ""
    }

    /// Quote one argument for `/bin/sh`, which is what `do shell script` invokes.
    /// Paths here come from `resolveBinary()` and from the user's home, both of
    /// which can contain spaces.
    static func shellQuote(_ argument: String) -> String {
        "'" + argument.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    /// AppleScript string literals need backslashes and quotes escaped — in that
    /// order, or the escapes we add get escaped again.
    private static func escapeForAppleScript(_ text: String) -> String {
        text.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
    }
}

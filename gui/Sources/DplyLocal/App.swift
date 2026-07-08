import SwiftUI
import AppKit

/// Entry point for the dply-local site manager.
///
/// This is a thin native front-end over the `dpl` CLI: every action shells out
/// to `dpl … --json` and renders the result. Building it as an SPM executable
/// keeps the whole project buildable from the command line (`swift run`).
///
/// Uses a single `Window` (not `WindowGroup`) plus a `@MainActor` app delegate
/// that promotes the process to a regular, focusable app — the combination SPM
/// SwiftUI executables need to reliably show a window and keep their run loop
/// alive.
@main
struct DplyLocalApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var store = Store()

    var body: some Scene {
        Window("Dply Local", id: "main") {
            ContentView()
                .environmentObject(store)
                .frame(minWidth: 900, idealWidth: 1100, minHeight: 560, idealHeight: 720)
                .task { await store.refreshAll() }
        }
        .defaultSize(width: 1100, height: 720)
        .windowResizability(.contentMinSize)
        .commands {
            CommandGroup(after: .toolbar) {
                Button("Refresh") { Task { await store.refreshAll() } }
                    .keyboardShortcut("r", modifiers: .command)
            }
        }

        Settings {
            SettingsView()
                .environmentObject(store)
        }
    }
}

/// Promotes the SPM executable to a normal foreground app and brings its
/// window to the front on launch.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}

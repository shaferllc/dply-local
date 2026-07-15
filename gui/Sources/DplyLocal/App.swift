import SwiftUI
import AppKit
import Combine
import Sparkle

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

    /// Sparkle self-updates. Only armed when running from a real bundle with a
    /// feed configured — a bare `swift run` binary has no Info.plist, and
    /// starting the updater there would just log errors.
    private let updaterController = SPUStandardUpdaterController(
        startingUpdater: Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") != nil,
        updaterDelegate: nil,
        userDriverDelegate: nil
    )

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
            CommandGroup(after: .appInfo) {
                CheckForUpdatesView(updater: updaterController.updater)
            }
            CommandGroup(after: .toolbar) {
                Button("Refresh") { Task { await store.refreshAll() } }
                    .keyboardShortcut("r", modifiers: .command)
            }
        }

        Settings {
            SettingsView()
                .environmentObject(store)
        }

        // PhpMon-style status-bar item: shows the default PHP version, with a
        // menu to switch PHP, manage sites/services, and open the app.
        MenuBarExtra {
            MenuBarContent().environmentObject(store)
        } label: {
            // Show the active PHP version in the status bar (PhpMon-style).
            Text(store.defaultPhp.map { "php \($0)" } ?? "php")
        }
        .menuBarExtraStyle(.menu)
    }
}

/// "Check for Updates…" menu item, enabled/disabled by the updater's own
/// state (Sparkle's documented SwiftUI pattern — the button must go inert
/// while a check or install is already in flight).
private final class CheckForUpdatesViewModel: ObservableObject {
    @Published var canCheckForUpdates = false
    init(updater: SPUUpdater) {
        updater.publisher(for: \.canCheckForUpdates).assign(to: &$canCheckForUpdates)
    }
}

struct CheckForUpdatesView: View {
    @ObservedObject private var model: CheckForUpdatesViewModel
    private let updater: SPUUpdater

    init(updater: SPUUpdater) {
        self.updater = updater
        self.model = CheckForUpdatesViewModel(updater: updater)
    }

    var body: some View {
        Button("Check for Updates…") { updater.checkForUpdates() }
            .disabled(!model.canCheckForUpdates)
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
        // Keep running in the menu bar after the window is closed.
        false
    }
}

import SwiftUI
import WebKit

/// Renders a captured email's HTML.
///
/// Two things a mail viewer must never do, both handled here:
///
/// - **Fetch remote content.** A captured email is untrusted input, and its
///   images are frequently tracking pixels. Loading one tells the sender that a
///   developer opened the mail, from this IP. Remote subresources are blocked by
///   a `WKContentRuleList` unless the reader explicitly allows them for that
///   message. `cid:` images are already inlined as `data:` URIs by
///   `dpl mail show --part html`, so embedded images still render.
/// - **Run scripts.** No email client executes JavaScript; neither do we.
///
/// Clicking a link opens it in the default browser rather than navigating this
/// view, so the preview can't be turned into a browser.
struct MailWebView: NSViewRepresentable {
    let html: String
    /// When true, remote images and stylesheets are permitted for this message.
    var allowRemote: Bool = false

    /// Block every http(s) subresource. The document itself is loaded from a
    /// string with no base URL, so it is `about:blank` and unaffected.
    private static let blockRules = """
    [{
      "trigger": {
        "url-filter": "^https?://",
        "resource-type": ["image", "style-sheet", "script", "font", "media", "raw", "svg-document"]
      },
      "action": { "type": "block" }
    }]
    """

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.defaultWebpagePreferences.allowsContentJavaScript = false

        let view = WKWebView(frame: .zero, configuration: config)
        view.setValue(false, forKey: "drawsBackground")
        view.navigationDelegate = context.coordinator
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {
        // Reload only when something actually changed: WKWebView reloads are
        // expensive and SwiftUI calls this on every parent redraw.
        guard context.coordinator.html != html || context.coordinator.allowRemote != allowRemote
        else { return }
        context.coordinator.html = html
        context.coordinator.allowRemote = allowRemote

        view.configuration.userContentController.removeAllContentRuleLists()

        guard !allowRemote else {
            view.loadHTMLString(html, baseURL: nil)
            return
        }

        WKContentRuleListStore.default()?.compileContentRuleList(
            forIdentifier: "dpl-block-remote",
            encodedContentRuleList: Self.blockRules
        ) { list, error in
            if let list {
                view.configuration.userContentController.add(list)
            } else if let error {
                // Fail closed: render nothing rather than leak a tracking pixel.
                view.loadHTMLString(
                    "<p style=\"font:14px -apple-system;color:#888;padding:24px\">"
                        + "Could not enable remote-content blocking, so this message was not "
                        + "rendered.<br><small>\(error.localizedDescription)</small></p>",
                    baseURL: nil
                )
                return
            }
            view.loadHTMLString(html, baseURL: nil)
        }
    }

    final class Coordinator: NSObject, WKNavigationDelegate {
        var html: String?
        var allowRemote: Bool?

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction,
            decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
        ) {
            // Only the initial loadHTMLString may navigate this view.
            if navigationAction.navigationType == .linkActivated,
               let url = navigationAction.request.url {
                NSWorkspace.shared.open(url)
                decisionHandler(.cancel)
                return
            }
            decisionHandler(.allow)
        }
    }
}

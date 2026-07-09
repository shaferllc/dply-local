import SwiftUI

/// How to point a project's mailer at dpl's sink, per framework.
///
/// The `MAIL_USERNAME` is the whole trick: nothing on the SMTP wire identifies
/// which site sent a message (`MAIL FROM` is `hello@example.com` for every
/// default Laravel app), so the username doubles as the mailbox name. Any
/// password works — the sink never checks it.
struct MailSetup: Identifiable, Hashable {
    let id: String
    let name: String
    /// Where the snippet goes, e.g. `.env`.
    let location: String
    let snippet: String
    let note: String

    static func all(host: String, port: Int, site: String) -> [MailSetup] {
        [
            MailSetup(
                id: "laravel",
                name: "Laravel",
                location: ".env",
                snippet: """
                MAIL_MAILER=smtp
                MAIL_HOST=\(host)
                MAIL_PORT=\(port)
                MAIL_USERNAME=\(site)
                MAIL_PASSWORD=dpl
                MAIL_ENCRYPTION=null
                MAIL_FROM_ADDRESS="hello@\(site).test"
                """,
                note: "Run `php artisan config:clear` afterwards if you've cached config."
            ),
            MailSetup(
                id: "symfony",
                name: "Symfony",
                location: ".env.local",
                snippet: """
                MAILER_DSN=smtp://\(site):dpl@\(host):\(port)
                """,
                note: "The username in the DSN is the mailbox. No TLS — the sink is plaintext on loopback."
            ),
            MailSetup(
                id: "wordpress",
                name: "WordPress",
                location: "wp-content/mu-plugins/dpl-mail.php",
                snippet: """
                <?php
                // Route wp_mail() through dpl's local sink.
                add_action('phpmailer_init', function ($mail) {
                    $mail->isSMTP();
                    $mail->Host       = '\(host)';
                    $mail->Port       = \(port);
                    $mail->SMTPAuth   = true;
                    $mail->Username   = '\(site)';   // becomes the mailbox
                    $mail->Password   = 'dpl';
                    $mail->SMTPSecure = '';
                    $mail->SMTPAutoTLS = false;
                });
                """,
                note: "Drop this in mu-plugins so it loads without activation."
            ),
            MailSetup(
                id: "php",
                name: "Raw PHP",
                location: "PHPMailer, or any SMTP client",
                snippet: """
                $mail = new PHPMailer\\PHPMailer\\PHPMailer();
                $mail->isSMTP();
                $mail->Host       = '\(host)';
                $mail->Port       = \(port);
                $mail->SMTPAuth   = true;
                $mail->Username   = '\(site)';   // becomes the mailbox
                $mail->Password   = 'dpl';
                $mail->SMTPAutoTLS = false;
                """,
                note: "PHP's bare mail() can't do SMTP auth, so it always lands in the unattributed mailbox."
            ),
        ]
    }
}

/// The Mail → Setup sheet: pick a framework, copy the snippet.
struct MailSetupSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    /// Pre-fill the snippets with a real site name when we know one.
    let site: String

    @State private var selected: String = "laravel"
    @State private var copied: String? = nil

    private var setups: [MailSetup] {
        MailSetup.all(host: "127.0.0.1", port: 1025, site: site)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Capture mail from your app").font(.title3.weight(.semibold))
                Spacer()
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            }
            .padding(16)

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text("dpl runs an SMTP sink on **127.0.0.1:1025**. Nothing it receives is ever delivered — messages are captured and listed here.")
                        .font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)

                    Label(
                        "Set MAIL_USERNAME to the site's name. That name becomes its mailbox, which is the only way dpl can tell one project's mail from another's. Any password works.",
                        systemImage: "tray.2"
                    )
                    .font(.callout)
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.violet.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))

                    Picker("", selection: $selected) {
                        ForEach(setups) { Text($0.name).tag($0.id) }
                    }
                    .pickerStyle(.segmented).labelsHidden()

                    if let setup = setups.first(where: { $0.id == selected }) {
                        VStack(alignment: .leading, spacing: 8) {
                            HStack {
                                Text(setup.location)
                                    .font(.caption.monospaced()).foregroundStyle(.secondary)
                                Spacer()
                                Button {
                                    copy(setup)
                                } label: {
                                    Label(copied == setup.id ? "Copied" : "Copy",
                                          systemImage: copied == setup.id ? "checkmark" : "doc.on.doc")
                                }
                                .buttonStyle(.bordered).controlSize(.small)
                            }

                            Text(setup.snippet)
                                .font(.system(.caption, design: .monospaced))
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(12)
                                .background(Color(nsColor: .textBackgroundColor),
                                            in: RoundedRectangle(cornerRadius: 8))

                            Text(setup.note).font(.caption).foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                .padding(16)
            }
        }
        .frame(width: 560, height: 560)
    }

    private func copy(_ setup: MailSetup) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(setup.snippet, forType: .string)
        copied = setup.id
        Task {
            try? await Task.sleep(for: .seconds(2))
            if copied == setup.id { copied = nil }
        }
    }
}

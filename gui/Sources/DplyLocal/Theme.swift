import SwiftUI

/// Design tokens for the app: the violet→cyan brand palette (matching the app
/// icon), reusable gradients, and view modifiers for the card / tile look used
/// throughout. Keeping these in one place is what makes the UI read as one
/// system.
enum Theme {
    // A dark, "lerd"-style palette: a coral/orange accent (the primary/`violet`
    // token — name kept so existing call sites keep working), warm secondary,
    // and a bright status green. The app runs in dark mode.
    static let violet = Color(red: 0.95, green: 0.35, blue: 0.22)   // accent (coral/orange)
    static let cyan = Color(red: 0.98, green: 0.55, blue: 0.27)     // warm secondary
    static let live = Color(red: 0.28, green: 0.86, blue: 0.55)     // status green

    /// The signature diagonal gradient (icon, accents, primary buttons).
    static let brand = LinearGradient(
        colors: [Color(red: 0.96, green: 0.30, blue: 0.20), Color(red: 0.98, green: 0.50, blue: 0.22)],
        startPoint: .topLeading,
        endPoint: .bottomTrailing
    )

    /// Card fill + border tuned for the dark UI.
    static let card = Color(red: 0.11, green: 0.11, blue: 0.12)
    static let hairline = Color.white.opacity(0.08)

    static let cardRadius: CGFloat = 12
    static let tileRadius: CGFloat = 9
}

extension View {
    /// A translucent, rounded "card" surface for grouping content.
    func cardSurface(padding: CGFloat = 16) -> some View {
        self
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.background.opacity(0.6), in: RoundedRectangle(cornerRadius: Theme.cardRadius))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .stroke(Color.primary.opacity(0.06), lineWidth: 1)
            )
    }
}

/// A rounded, gradient-filled icon tile — the leading accent on list rows and
/// detail headers.
struct GradientTile: View {
    let systemImage: String
    var size: CGFloat = 34
    /// When false, uses a muted neutral fill (e.g. a stopped/secondary item).
    var active: Bool = true

    var body: some View {
        RoundedRectangle(cornerRadius: Theme.tileRadius, style: .continuous)
            .fill(active ? AnyShapeStyle(Theme.brand) : AnyShapeStyle(Color.secondary.opacity(0.22)))
            .frame(width: size, height: size)
            .overlay(
                Image(systemName: systemImage)
                    .font(.system(size: size * 0.44, weight: .semibold))
                    .foregroundStyle(active ? .white : Color.secondary)
            )
            .shadow(color: active ? Theme.violet.opacity(0.28) : .clear, radius: 5, y: 2)
    }
}

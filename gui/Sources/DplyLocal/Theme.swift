import SwiftUI

/// Design tokens for the app: the violet→cyan brand palette (matching the app
/// icon), reusable gradients, and view modifiers for the card / tile look used
/// throughout. Keeping these in one place is what makes the UI read as one
/// system.
enum Theme {
    static let violet = Color(red: 0.42, green: 0.25, blue: 0.86)
    static let cyan = Color(red: 0.13, green: 0.80, blue: 0.93)
    static let live = Color(red: 0.20, green: 0.82, blue: 0.51)

    /// The signature diagonal gradient (icon, accents, primary buttons).
    static let brand = LinearGradient(
        colors: [violet, cyan],
        startPoint: .topLeading,
        endPoint: .bottomTrailing
    )

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

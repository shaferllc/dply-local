#!/usr/bin/env swift
// Generates AppIcon.icns from a programmatic design.
// Usage: swift make-icon.swift  (run from the gui/ dir)
//
// Design: a violet→cyan squircle holding a white "browser window" (the local
// site you're serving) with a top chrome bar, an address pill, content bars in
// the accent gradient, and a green "live" dot signalling a running site.

import AppKit
import Foundation

let here = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
let iconset = here.appendingPathComponent("AppIcon.iconset")
try? FileManager.default.removeItem(at: iconset)
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

let sizes: [(String, Int)] = [
    ("icon_16x16", 16), ("icon_16x16@2x", 32),
    ("icon_32x32", 32), ("icon_32x32@2x", 64),
    ("icon_128x128", 128), ("icon_128x128@2x", 256),
    ("icon_256x256", 256), ("icon_256x256@2x", 512),
    ("icon_512x512", 512), ("icon_512x512@2x", 1024),
]

// Brand palette.
let violet = NSColor(red: 0.42, green: 0.25, blue: 0.86, alpha: 1) // #6C40DB
let cyan = NSColor(red: 0.13, green: 0.80, blue: 0.93, alpha: 1)   // #21CCED
let green = NSColor(red: 0.20, green: 0.82, blue: 0.51, alpha: 1)  // live dot

func makePNG(size px: Int) -> Data? {
    let pf = CGFloat(px)
    guard let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 32)
    else { return nil }
    rep.size = NSSize(width: pf, height: pf)

    NSGraphicsContext.saveGraphicsState()
    defer { NSGraphicsContext.restoreGraphicsState() }
    guard let ctx = NSGraphicsContext(bitmapImageRep: rep) else { return nil }
    NSGraphicsContext.current = ctx

    // Squircle background: violet → cyan diagonal gradient.
    let radius = pf * 0.225
    let squircle = NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: pf, height: pf),
                                xRadius: radius, yRadius: radius)
    squircle.addClip()
    NSGradient(colors: [violet, cyan])!.draw(
        in: NSRect(x: 0, y: 0, width: pf, height: pf), angle: -50)

    // Soft top highlight for depth.
    let hi = NSGradient(colors: [NSColor(white: 1, alpha: 0.16), NSColor(white: 1, alpha: 0)])!
    hi.draw(in: NSRect(x: 0, y: pf * 0.5, width: pf, height: pf * 0.5), angle: -90)

    // Browser window card.
    let inset = pf * 0.215
    let card = NSRect(x: inset, y: inset, width: pf - 2 * inset, height: pf - 2 * inset)
    let cardR = pf * 0.06
    // Drop shadow.
    NSColor(white: 0, alpha: 0.20).setFill()
    NSBezierPath(roundedRect: card.offsetBy(dx: 0, dy: -pf * 0.016),
                 xRadius: cardR, yRadius: cardR).fill()
    NSColor.white.setFill()
    NSBezierPath(roundedRect: card, xRadius: cardR, yRadius: cardR).fill()

    // Chrome bar (top strip of the window).
    let barH = card.height * 0.22
    let bar = NSRect(x: card.minX, y: card.maxY - barH, width: card.width, height: barH)
    NSGraphicsContext.saveGraphicsState()
    NSBezierPath(roundedRect: card, xRadius: cardR, yRadius: cardR).addClip()
    NSColor(white: 0.96, alpha: 1).setFill()
    NSBezierPath(rect: bar).fill()
    NSColor(white: 0.86, alpha: 1).setFill()
    NSBezierPath(rect: NSRect(x: card.minX, y: bar.minY, width: card.width, height: max(1, pf * 0.004))).fill()
    NSGraphicsContext.restoreGraphicsState()

    // Traffic-light dots.
    let dot = card.width * 0.05
    let dy = bar.midY - dot / 2
    for (i, c) in [violet, cyan, green].enumerated() {
        c.setFill()
        let x = card.minX + card.width * 0.10 + CGFloat(i) * dot * 2.1
        NSBezierPath(ovalIn: NSRect(x: x, y: dy, width: dot, height: dot)).fill()
    }

    // Content: an address pill + two accent bars suggesting a rendered page.
    let pad = card.width * 0.12
    let contentTop = bar.minY - card.height * 0.10
    // address pill
    NSColor(white: 0.90, alpha: 1).setFill()
    let pillH = card.height * 0.10
    NSBezierPath(roundedRect: NSRect(x: card.minX + pad, y: contentTop - pillH,
                                     width: card.width - 2 * pad, height: pillH),
                 xRadius: pillH / 2, yRadius: pillH / 2).fill()
    // two bars in the brand gradient
    func contentBar(_ yFrac: CGFloat, _ wFrac: CGFloat) {
        let h = card.height * 0.11
        let y = card.minY + card.height * yFrac
        let r = NSRect(x: card.minX + pad, y: y, width: (card.width - 2 * pad) * wFrac, height: h)
        NSGraphicsContext.saveGraphicsState()
        NSBezierPath(roundedRect: r, xRadius: h / 2, yRadius: h / 2).addClip()
        NSGradient(colors: [violet, cyan])!.draw(in: r, angle: 0)
        NSGraphicsContext.restoreGraphicsState()
    }
    contentBar(0.30, 0.85)
    contentBar(0.13, 0.55)

    // "Live" dot with a soft halo at the window's bottom-right.
    let live = NSPoint(x: card.maxX - pad * 0.7, y: card.minY + card.height * 0.30)
    let lr = card.width * 0.055
    green.withAlphaComponent(0.25).setFill()
    NSBezierPath(ovalIn: NSRect(x: live.x - lr * 2, y: live.y - lr * 2, width: lr * 4, height: lr * 4)).fill()
    green.setFill()
    NSBezierPath(ovalIn: NSRect(x: live.x - lr, y: live.y - lr, width: lr * 2, height: lr * 2)).fill()

    return rep.representation(using: .png, properties: [:])
}

for (name, px) in sizes {
    guard let data = makePNG(size: px) else { continue }
    try data.write(to: iconset.appendingPathComponent("\(name).png"))
}

let proc = Process()
proc.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
proc.arguments = ["-c", "icns", iconset.path, "-o", here.appendingPathComponent("AppIcon.icns").path]
try proc.run()
proc.waitUntilExit()
print("Wrote AppIcon.icns")

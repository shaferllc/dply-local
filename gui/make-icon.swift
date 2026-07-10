#!/usr/bin/env swift
// Generates AppIcon.icns from a programmatic design.
// Usage: swift make-icon.swift  (run from the gui/ dir)
//
// Design: matches the dply marketing site's logo (local.dply.io) — an orange
// gradient squircle (#F97B3D → #F4552E) holding a white "waveform" stroke. The
// path and geometry mirror resources/public/logo.svg one-for-one, scaled to px.

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

// Brand gradient (site logo's linear stops).
let orangeLight = NSColor(red: 0.976, green: 0.482, blue: 0.239, alpha: 1) // #F97B3D
let orangeDeep = NSColor(red: 0.957, green: 0.333, blue: 0.180, alpha: 1)  // #F4552E

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

    // The SVG is authored on a 64-unit canvas; scale everything by px/64. SVG y
    // is top-down, AppKit's bitmap context is bottom-up, so flip y here.
    let s = pf / 64
    func pt(_ x: CGFloat, _ y: CGFloat) -> NSPoint { NSPoint(x: x * s, y: (64 - y) * s) }

    // Squircle background: orange diagonal gradient (rx=15 in the 64 canvas).
    let radius = pf * 15 / 64
    let squircle = NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: pf, height: pf),
                                xRadius: radius, yRadius: radius)
    squircle.addClip()
    NSGradient(colors: [orangeLight, orangeDeep])!.draw(
        in: NSRect(x: 0, y: 0, width: pf, height: pf), angle: -45)

    // Soft top highlight for a little depth, kept subtle so it stays flat.
    let hi = NSGradient(colors: [NSColor(white: 1, alpha: 0.14), NSColor(white: 1, alpha: 0)])!
    hi.draw(in: NSRect(x: 0, y: pf * 0.5, width: pf, height: pf * 0.5), angle: -90)

    // White waveform — the exact path from logo.svg:
    //   M12 34 C18 20 24 20 30 32 C36 44 42 44 48 30 L52 30
    let wave = NSBezierPath()
    wave.lineWidth = 4.5 * s
    wave.lineCapStyle = .round
    wave.lineJoinStyle = .round
    wave.move(to: pt(12, 34))
    wave.curve(to: pt(30, 32), controlPoint1: pt(18, 20), controlPoint2: pt(24, 20))
    wave.curve(to: pt(48, 30), controlPoint1: pt(36, 44), controlPoint2: pt(42, 44))
    wave.line(to: pt(52, 30))
    NSColor.white.setStroke()
    wave.stroke()

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

//! Draws the BurnCraft icon and, on Windows, embeds it in the executable.
//!
//! The icon is drawn from vector shapes at each size (so small sizes stay sharp):
//! a flame on a dark rounded tile, struck by a cyan laser beam. Outputs in OUT_DIR:
//! `burncraft.ico` (all sizes), `icon-256.png` (preview) and `icon-256.rgba` (window icon).

use std::path::Path;
use tiny_skia::{Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, SpreadMode, Stroke, Transform};

const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = std::env::var("OUT_DIR").unwrap();
    let out = Path::new(&out);

    let mut frames = Vec::new();
    for size in ICO_SIZES {
        let png = draw(size).encode_png().expect("encode icon png");
        frames.push(image::codecs::ico::IcoFrame::with_encoded(png, size, size, image::ExtendedColorType::Rgba8).expect("icon frame"));
    }
    let ico_path = out.join("burncraft.ico");
    let file = std::fs::File::create(&ico_path).expect("create icon");
    image::codecs::ico::IcoEncoder::new(file).encode_images(&frames).expect("write icon");

    let big = draw(256);
    big.save_png(out.join("icon-256.png")).expect("write preview");
    for size in [16, 32] {
        draw(size).save_png(out.join(format!("icon-{size}.png"))).expect("write preview");
    }
    // Straight (non-premultiplied) RGBA for the window icon.
    let rgba: Vec<u8> = big.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect();
    std::fs::write(out.join("icon-256.rgba"), rgba).expect("write rgba");

    // The camera uses Windows Media Foundation, which "N" editions of Windows don't have. Load it
    // only when the camera is used, so BurnCraft still starts there (the camera says it's missing).
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        for dll in ["mf.dll", "mfplat.dll", "mfreadwrite.dll"] {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
        // An 8 MB main-thread stack (Windows' default is 1 MB). Running out of stack ends a
        // program instantly with no message, so leave plenty of room.
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico_path.to_str().unwrap());
        res.set("FileDescription", "BurnCraft laser software");
        res.set("ProductName", "BurnCraft");
        if let Err(e) = res.compile() {
            // Not fatal: the app still builds, just without the Explorer icon.
            println!("cargo:warning=could not embed the icon: {e}");
        }
    }
}

fn rgb(hex: u32) -> Color {
    Color::from_rgba8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, 255)
}

fn rgba(hex: u32, alpha: f32) -> Color {
    let mut c = rgb(hex);
    c.set_alpha(alpha);
    c
}

fn vertical(top: Color, bottom: Color, y0: f32, y1: f32) -> Paint<'static> {
    let stops = vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)];
    let shader = LinearGradient::new(Point::from_xy(0.0, y0), Point::from_xy(0.0, y1), stops, SpreadMode::Pad, Transform::identity()).unwrap();
    Paint { anti_alias: true, shader, ..Paint::default() }
}

fn solid(c: Color) -> Paint<'static> {
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    paint.set_color(c);
    paint
}

/// The icon at `size` px. Geometry is designed on a 256 grid.
fn draw(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).unwrap();
    let t = Transform::from_scale(size as f32 / 256.0, size as f32 / 256.0);
    let small = size <= 24;

    // Tile: rounded square, charcoal gradient, faint rim.
    let (inset, r) = (if small { 4.0 } else { 8.0 }, 52.0);
    let tile = rounded_rect(inset, inset, 256.0 - 2.0 * inset, 256.0 - 2.0 * inset, r);
    pm.fill_path(&tile, &vertical(rgb(0x2E3446), rgb(0x141720), 0.0, 256.0), FillRule::Winding, t, None);
    if !small {
        pm.stroke_path(&tile, &solid(rgba(0xFFFFFF, 0.10)), &Stroke { width: 4.0, ..Stroke::default() }, t, None);
    }

    // Engraved work line under the flame.
    let mut pb = PathBuilder::new();
    pb.move_to(58.0, 214.0);
    pb.line_to(198.0, 214.0);
    let line = pb.finish().unwrap();
    pm.stroke_path(&line, &solid(rgba(0xFF9A3C, 0.55)), &Stroke { width: if small { 12.0 } else { 7.0 }, line_cap: tiny_skia::LineCap::Round, ..Stroke::default() }, t, None);

    // Outer flame.
    let mut pb = PathBuilder::new();
    pb.move_to(128.0, 210.0);
    pb.cubic_to(86.0, 210.0, 62.0, 182.0, 64.0, 148.0);
    pb.cubic_to(66.0, 118.0, 88.0, 100.0, 94.0, 66.0);
    pb.cubic_to(110.0, 84.0, 116.0, 100.0, 114.0, 120.0);
    pb.cubic_to(132.0, 104.0, 146.0, 78.0, 138.0, 34.0);
    pb.cubic_to(174.0, 64.0, 194.0, 106.0, 192.0, 148.0);
    pb.cubic_to(190.0, 184.0, 166.0, 210.0, 128.0, 210.0);
    pb.close();
    let outer = pb.finish().unwrap();
    pm.fill_path(&outer, &vertical(rgb(0xFFB02E), rgb(0xF0361C), 40.0, 210.0), FillRule::Winding, t, None);

    // Inner flame.
    let mut pb = PathBuilder::new();
    pb.move_to(128.0, 204.0);
    pb.cubic_to(106.0, 204.0, 94.0, 188.0, 96.0, 168.0);
    pb.cubic_to(98.0, 150.0, 112.0, 140.0, 116.0, 122.0);
    pb.cubic_to(126.0, 134.0, 130.0, 146.0, 128.0, 160.0);
    pb.cubic_to(140.0, 150.0, 148.0, 136.0, 146.0, 116.0);
    pb.cubic_to(162.0, 134.0, 168.0, 152.0, 166.0, 170.0);
    pb.cubic_to(164.0, 192.0, 150.0, 204.0, 128.0, 204.0);
    pb.close();
    let inner = pb.finish().unwrap();
    pm.fill_path(&inner, &vertical(rgb(0xFFF6C4), rgb(0xFFC93A), 116.0, 204.0), FillRule::Winding, t, None);

    // Laser beam from the top right, over the flame, striking the work line: glow, colour, bright core.
    if !small {
        let (from, to) = ((210.0, 30.0), (140.0, 212.0));
        let mut pb = PathBuilder::new();
        pb.move_to(from.0, from.1);
        pb.line_to(to.0, to.1);
        let beam = pb.finish().unwrap();
        let round = |width| Stroke { width, line_cap: tiny_skia::LineCap::Round, ..Stroke::default() };
        pm.stroke_path(&beam, &solid(rgba(0x35E0FF, 0.25)), &round(20.0), t, None);
        pm.stroke_path(&beam, &solid(rgba(0x35E0FF, 0.9)), &round(8.0), t, None);
        pm.stroke_path(&beam, &solid(rgba(0xF2FEFF, 1.0)), &round(3.0), t, None);
        // Spark where it hits.
        for (radius, alpha) in [(18.0, 0.25), (10.0, 0.6), (5.0, 1.0)] {
            let mut pb = PathBuilder::new();
            pb.push_circle(to.0, to.1, radius);
            pm.fill_path(&pb.finish().unwrap(), &solid(rgba(0xE8FCFF, alpha)), FillRule::Winding, t, None);
        }
    }
    pm
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> tiny_skia::Path {
    // Cubic approximation of quarter circles.
    let k = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().unwrap()
}

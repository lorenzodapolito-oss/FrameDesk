//! Report HTML pastello con miniature incorporate (PNG scritto a mano, senza dipendenze).

use crate::model::{Thumb, VideoResult};

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::with_capacity(data.len() / 3 * 4 + 4);
    for ch in data.chunks(3) {
        let b = [
            ch.first().copied().unwrap_or(0),
            ch.get(1).copied().unwrap_or(0),
            ch.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let at = |s: u32| char::from(T.get(((n >> s) & 63) as usize).copied().unwrap_or(b'='));
        o.push(at(18));
        o.push(at(12));
        o.push(if ch.len() > 1 { at(6) } else { '=' });
        o.push(if ch.len() > 2 { at(0) } else { '=' });
    }
    o
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for d in data {
        a = (a + u32::from(*d)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend((body.len() as u32).to_be_bytes());
    let mut c = kind.to_vec();
    c.extend_from_slice(body);
    out.extend(&c);
    out.extend(crc32(&c).to_be_bytes());
}

/// PNG RGB 8 bit non compresso (blocchi "stored"). None se le misure non tornano.
pub fn png_rgb(w: usize, h: usize, rgb: &[u8]) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || rgb.len() != w.checked_mul(h)?.checked_mul(3)? || w > 4096 || h > 4096 {
        return None;
    }
    let mut raw = Vec::with_capacity(h * (w * 3 + 1));
    for row in rgb.chunks_exact(w * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let n = raw.chunks(65535).count();
    for (i, blk) in raw.chunks(65535).enumerate() {
        z.push(u8::from(i + 1 == n));
        z.extend((blk.len() as u16).to_le_bytes());
        z.extend((!(blk.len() as u16)).to_le_bytes());
        z.extend_from_slice(blk);
    }
    z.extend(adler32(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend((w as u32).to_be_bytes());
    ihdr.extend((h as u32).to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

fn img(t: &Option<Thumb>, title: &str) -> String {
    let Some(t) = t else { return String::new() };
    let Some(png) = png_rgb(t.w, t.h, &t.rgb) else {
        return String::new();
    };
    format!(
        "<figure><img alt=\"{a}\" src=\"data:image/png;base64,{}\"><figcaption>{a}<br>fotogramma {} · punteggio {:.0}</figcaption></figure>",
        base64(&png),
        t.frame_idx,
        t.score,
        a = esc(title)
    )
}

fn rgb_css(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

pub fn html(results: &[VideoResult], when: &str) -> String {
    let mut o = String::from(
        "<!DOCTYPE html><html lang=\"it\"><head><meta charset=\"utf-8\"><title>FrameDesk - report</title><style>\
body{font-family:Segoe UI,sans-serif;background:#D2E4FA;color:#22315E;margin:0;padding:24px}\
h1{color:#4F7CD6}table{border-collapse:collapse;width:100%;background:#E0ECFC;border-radius:12px}\
th,td{padding:8px 10px;text-align:left}th{background:#B4D0F4}tr:nth-child(even) td{background:#C8DCF6}\
.card{background:#E0ECFC;border-radius:14px;padding:16px;margin:18px 0}\
.badge{display:inline-block;padding:2px 10px;border-radius:10px;font-weight:600}\
figure{display:inline-block;margin:6px 12px 6px 0;text-align:center;font-size:12px}\
img{border-radius:8px;display:block}.err{color:#BC3C58}</style></head><body>",
    );
    o.push_str(&format!(
        "<h1>FrameDesk di Lorenzo D'Apolito</h1><p>Report del {} · {} video</p>",
        esc(when),
        results.len()
    ));
    o.push_str("<table><tr><th>Video</th><th>Durata</th><th>Risoluzione</th><th>Codec</th><th>Mbps</th><th>Purezza</th><th>Densità</th><th>Stabilità</th><th>Punteggio</th><th>Voto</th></tr>");
    for r in results {
        match (&r.error, r.grade) {
            (Some(e), _) => o.push_str(&format!(
                "<tr><td>{}</td><td colspan=\"9\" class=\"err\">{}</td></tr>",
                esc(&r.name),
                esc(e)
            )),
            (None, g) => {
                let (label, col) = g.map_or(("-", "#B4D0F4".to_string()), |g| {
                    (g.label(), rgb_css(g.rgb()))
                });
                o.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}x{}</td><td>{}</td><td>{:.2}</td><td>{:.0}</td><td>{:.0}</td><td>{:.0}</td><td><b>{:.1}</b></td><td><span class=\"badge\" style=\"background:{col}\">{label}</span></td></tr>",
                    esc(&r.name), esc(&r.duration_text), r.width, r.height, esc(&r.codec), r.bitrate_mbps,
                    r.purity_norm, r.density_norm, r.stability_norm, r.score
                ));
            }
        }
    }
    o.push_str("</table>");
    for r in results.iter().filter(|r| r.error.is_none()) {
        o.push_str(&format!(
            "<div class=\"card\"><h2>{}</h2><p>{} · {} · {} fotogrammi analizzati su {} · {} · {}{}</p>",
            esc(&r.name),
            esc(&r.size_text),
            esc(&r.duration_text),
            r.analyzed_frames,
            r.total_frames,
            esc(&r.pix_fmt),
            esc(&r.hdr_type),
            if r.has_anchors { " · confrontato con le ancore" } else { "" }
        ));
        o.push_str(&format!(
            "<p>Blocchettatura {:.2} · rapporto dettaglio {:.3} · sospetto upscaling {:.0}% · sfarfallio {:.2} · fermi immagine {:.0}%</p>",
            r.blockiness, r.fft_ratio, r.upscale_score * 100.0, r.flicker, r.freeze_ratio * 100.0
        ));
        o.push_str(&img(&r.thumbs.best, "Migliore"));
        o.push_str(&img(&r.thumbs.worst, "Peggiore"));
        o.push_str(&img(&r.thumbs.upscale_suspect, "Sospetto upscaling"));
        o.push_str("</div>");
    }
    o.push_str("</body></html>");
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Grade;

    #[test]
    fn escapes_html() {
        assert_eq!(
            esc("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn png_has_signature_and_rejects_bad_sizes() {
        let p = png_rgb(2, 2, &[10; 12]).unwrap();
        assert_eq!(&p[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert!(p.ends_with(&[0xAE, 0x42, 0x60, 0x82]));
        assert!(png_rgb(0, 2, &[]).is_none());
        assert!(png_rgb(2, 2, &[0; 5]).is_none());
        // Più di un blocco "stored".
        assert!(png_rgb(200, 200, &vec![5; 200 * 200 * 3]).is_some());
    }

    #[test]
    fn report_escapes_names_and_shows_errors() {
        let mut ok = VideoResult {
            name: "<b>x</b>.mp4".into(),
            score: 80.0,
            grade: Some(Grade::MoltoBuono),
            ..VideoResult::default()
        };
        ok.thumbs.best = Some(Thumb {
            frame_idx: 3,
            score: 9.0,
            purity: 1.0,
            density: 1.0,
            upscale: 0.0,
            w: 2,
            h: 2,
            rgb: vec![9; 12],
        });
        let bad = VideoResult::failed("c.mp4", "Video non apribile");
        let h = html(&[ok, bad], "oggi");
        assert!(!h.contains("<b>x</b>"));
        assert!(
            h.contains("&lt;b&gt;x")
                && h.contains("MOLTO BUONO")
                && h.contains("Video non apribile")
        );
        assert!(h.contains("data:image/png;base64,"));
    }

    #[test]
    fn report_has_no_white_or_gray_hex_colors() {
        let h = html(&[], "x");
        for bad in [
            "#FFF", "#fff", "#FFFFFF", "#ffffff", "white", "gray", "grey",
        ] {
            assert!(!h.contains(bad), "{bad}");
        }
    }
}

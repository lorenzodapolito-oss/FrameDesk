//! Elaborazione d'immagine scritta da zero (senza OpenCV): grigio, Sobel, Canny, dilatazione,
//! ridimensionamento e FFT. Tutto lavora su dimensioni piccole (i fotogrammi si riducono a 640
//! pixel al massimo) e non va mai in panico: gli indici fuori dai limiti si riportano dentro.

use rustfft::{num_complex::Complex, FftPlanner};

/// Immagine in toni di grigio, 8 bit.
#[derive(Clone, Debug, PartialEq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub d: Vec<u8>,
}

/// Immagine a colori RGB, 8 bit per canale.
#[derive(Clone, Debug, PartialEq)]
pub struct Rgb {
    pub w: usize,
    pub h: usize,
    pub d: Vec<u8>,
}

impl Rgb {
    /// Crea un'immagine solo se i dati hanno esattamente la lunghezza giusta.
    pub fn new(w: usize, h: usize, d: Vec<u8>) -> Option<Self> {
        let len = w.checked_mul(h)?.checked_mul(3)?;
        (w > 0 && h > 0 && d.len() == len).then_some(Self { w, h, d })
    }

    /// Grigio come OpenCV: 0.299 R + 0.587 G + 0.114 B.
    pub fn to_gray(&self) -> Gray {
        let d = self
            .d
            .chunks_exact(3)
            .map(|p| {
                let (r, g, b) = (f32::from(p[0]), f32::from(p[1]), f32::from(p[2]));
                (0.299 * r + 0.587 * g + 0.114 * b)
                    .round()
                    .clamp(0.0, 255.0) as u8
            })
            .collect();
        Gray {
            w: self.w,
            h: self.h,
            d,
        }
    }

    /// Riduzione a media di area (come `INTER_AREA`).
    pub fn resize_area(&self, dw: usize, dh: usize) -> Option<Rgb> {
        if dw == 0 || dh == 0 {
            return None;
        }
        let mut out = vec![0u8; dw.checked_mul(dh)?.checked_mul(3)?];
        for c in 0..3 {
            let plane: Vec<u8> = self.d.iter().skip(c).step_by(3).copied().collect();
            let g = Gray {
                w: self.w,
                h: self.h,
                d: plane,
            };
            let r = g.resize_area(dw, dh)?;
            for (i, v) in r.d.iter().enumerate() {
                if let Some(o) = out.get_mut(i * 3 + c) {
                    *o = *v;
                }
            }
        }
        Rgb::new(dw, dh, out)
    }
}

impl Gray {
    pub fn new(w: usize, h: usize, d: Vec<u8>) -> Option<Self> {
        let len = w.checked_mul(h)?;
        (w > 0 && h > 0 && d.len() == len).then_some(Self { w, h, d })
    }

    pub fn at(&self, x: usize, y: usize) -> u8 {
        self.d.get(y * self.w + x).copied().unwrap_or(0)
    }

    /// Luminanza media (0-255).
    pub fn mean(&self) -> f64 {
        mean_u8(&self.d)
    }

    /// Deviazione standard (popolazione, come NumPy).
    pub fn std(&self) -> f64 {
        std_u8(&self.d)
    }

    /// Ritaglio dei bordi: toglie `m` pixel da ogni lato.
    pub fn crop_margin(&self, m: usize) -> Option<Gray> {
        if m == 0 {
            return Some(self.clone());
        }
        let nw = self.w.checked_sub(m.checked_mul(2)?)?;
        let nh = self.h.checked_sub(m.checked_mul(2)?)?;
        if nw == 0 || nh == 0 {
            return None;
        }
        let mut d = Vec::with_capacity(nw * nh);
        for y in m..m + nh {
            let start = y * self.w + m;
            d.extend_from_slice(self.d.get(start..start + nw)?);
        }
        Gray::new(nw, nh, d)
    }

    /// Ritaglio centrale di lato `size` (o meno se l'immagine è più piccola).
    pub fn crop_center(&self, cw: usize, ch: usize) -> Option<Gray> {
        let cw = cw.min(self.w);
        let ch = ch.min(self.h);
        let x0 = (self.w / 2).saturating_sub(cw / 2);
        let y0 = (self.h / 2).saturating_sub(ch / 2);
        let x0 = x0.min(self.w.saturating_sub(cw));
        let y0 = y0.min(self.h.saturating_sub(ch));
        let mut d = Vec::with_capacity(cw * ch);
        for y in y0..y0 + ch {
            let start = y * self.w + x0;
            d.extend_from_slice(self.d.get(start..start + cw)?);
        }
        Gray::new(cw, ch, d)
    }

    /// Dimezza larghezza e altezza con la media di 2x2.
    pub fn half(&self) -> Option<Gray> {
        self.resize_area((self.w / 2).max(1), (self.h / 2).max(1))
    }

    /// Riduzione a media di area con pesi frazionari (come `INTER_AREA`).
    pub fn resize_area(&self, dw: usize, dh: usize) -> Option<Gray> {
        if dw == 0 || dh == 0 || self.w == 0 || self.h == 0 {
            return None;
        }
        let sx = self.w as f64 / dw as f64;
        let sy = self.h as f64 / dh as f64;
        let mut out = Vec::with_capacity(dw.checked_mul(dh)?);
        for oy in 0..dh {
            let (y0, y1) = (oy as f64 * sy, (oy as f64 + 1.0) * sy);
            for ox in 0..dw {
                let (x0, x1) = (ox as f64 * sx, (ox as f64 + 1.0) * sx);
                let mut acc = 0.0;
                let mut wsum = 0.0;
                let ys = y0.floor() as usize;
                let ye = (y1.ceil() as usize).min(self.h);
                let xs = x0.floor() as usize;
                let xe = (x1.ceil() as usize).min(self.w);
                for yy in ys..ye {
                    let wy = (yy as f64 + 1.0).min(y1) - (yy as f64).max(y0);
                    for xx in xs..xe {
                        let wx = (xx as f64 + 1.0).min(x1) - (xx as f64).max(x0);
                        let wgt = wx * wy;
                        acc += f64::from(self.at(xx, yy)) * wgt;
                        wsum += wgt;
                    }
                }
                let v = if wsum > 0.0 { acc / wsum } else { 0.0 };
                out.push(v.round().clamp(0.0, 255.0) as u8);
            }
        }
        Gray::new(dw, dh, out)
    }

    /// Ingrandimento bicubico (come `INTER_CUBIC`, coefficiente -0,75, bordi replicati).
    pub fn resize_cubic(&self, dw: usize, dh: usize) -> Option<Gray> {
        if dw == 0 || dh == 0 || self.w == 0 || self.h == 0 {
            return None;
        }
        let sx = self.w as f64 / dw as f64;
        let sy = self.h as f64 / dh as f64;
        let weights = |t: f64| -> [f64; 4] {
            let a = -0.75;
            let f = |x: f64| -> f64 {
                let x = x.abs();
                if x <= 1.0 {
                    ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
                } else if x < 2.0 {
                    ((a * x - 5.0 * a) * x + 8.0 * a) * x - 4.0 * a
                } else {
                    0.0
                }
            };
            [f(t + 1.0), f(t), f(1.0 - t), f(2.0 - t)]
        };
        let clamp = |v: i64, n: usize| -> usize { v.clamp(0, n as i64 - 1) as usize };
        let mut out = Vec::with_capacity(dw.checked_mul(dh)?);
        for oy in 0..dh {
            let fy = (oy as f64 + 0.5) * sy - 0.5;
            let iy = fy.floor();
            let wy = weights(fy - iy);
            for ox in 0..dw {
                let fx = (ox as f64 + 0.5) * sx - 0.5;
                let ix = fx.floor();
                let wx = weights(fx - ix);
                let mut acc = 0.0;
                for (j, wyj) in wy.iter().enumerate() {
                    let yy = clamp(iy as i64 - 1 + j as i64, self.h);
                    for (i, wxi) in wx.iter().enumerate() {
                        let xx = clamp(ix as i64 - 1 + i as i64, self.w);
                        acc += f64::from(self.at(xx, yy)) * wxi * wyj;
                    }
                }
                out.push(acc.round().clamp(0.0, 255.0) as u8);
            }
        }
        Gray::new(dw, dh, out)
    }
}

pub fn mean_u8(d: &[u8]) -> f64 {
    if d.is_empty() {
        return 0.0;
    }
    d.iter().map(|v| f64::from(*v)).sum::<f64>() / d.len() as f64
}

pub fn std_u8(d: &[u8]) -> f64 {
    if d.is_empty() {
        return 0.0;
    }
    let m = mean_u8(d);
    let var = d
        .iter()
        .map(|v| {
            let x = f64::from(*v) - m;
            x * x
        })
        .sum::<f64>()
        / d.len() as f64;
    var.sqrt()
}

/// Media di un insieme di numeri (0 se vuoto).
pub fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// Deviazione standard di popolazione (0 se vuoto).
pub fn std(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let m = mean(v);
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / v.len() as f64).sqrt()
}

/// Correlazione di Pearson; 0 se uno dei due insiemi è costante o se le lunghezze differiscono.
pub fn pearson(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.len() < 2 {
        return 0.0;
    }
    let (ma, mb) = (mean(a), mean(b));
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        sab += (x - ma) * (y - mb);
        saa += (x - ma) * (x - ma);
        sbb += (y - mb) * (y - mb);
    }
    let den = (saa * sbb).sqrt();
    if den <= f64::EPSILON {
        return 0.0;
    }
    let r = sab / den;
    if r.is_finite() {
        r
    } else {
        0.0
    }
}

/// Indice con bordo riflesso (come `BORDER_REFLECT_101`).
fn reflect(i: i64, n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let n = n as i64;
    let mut i = i;
    if i < 0 {
        i = -i;
    }
    if i >= n {
        i = 2 * n - 2 - i;
    }
    i.clamp(0, n - 1) as usize
}

/// Gradienti di Sobel 3x3 (orizzontale, verticale) come numeri decimali.
pub fn sobel(g: &Gray) -> (Vec<f64>, Vec<f64>) {
    let (w, h) = (g.w, g.h);
    let mut gx = vec![0.0; w * h];
    let mut gy = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = |dx: i64, dy: i64| -> f64 {
                f64::from(g.at(reflect(x as i64 + dx, w), reflect(y as i64 + dy, h)))
            };
            let sx = (p(1, -1) + 2.0 * p(1, 0) + p(1, 1)) - (p(-1, -1) + 2.0 * p(-1, 0) + p(-1, 1));
            let sy = (p(-1, 1) + 2.0 * p(0, 1) + p(1, 1)) - (p(-1, -1) + 2.0 * p(0, -1) + p(1, -1));
            if let Some(v) = gx.get_mut(y * w + x) {
                *v = sx;
            }
            if let Some(v) = gy.get_mut(y * w + x) {
                *v = sy;
            }
        }
    }
    (gx, gy)
}

/// Contorni di Canny (soglie `lo` e `hi`, norma L1 come OpenCV di default).
pub fn canny(g: &Gray, lo: f64, hi: f64) -> Vec<bool> {
    let (w, h) = (g.w, g.h);
    let (gx, gy) = sobel(g);
    let mag: Vec<f64> = gx.iter().zip(&gy).map(|(a, b)| a.abs() + b.abs()).collect();
    let m = |x: i64, y: i64| -> f64 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            0.0
        } else {
            mag.get(y as usize * w + x as usize).copied().unwrap_or(0.0)
        }
    };
    // Soppressione dei non massimi lungo la direzione del gradiente.
    let mut strong = vec![false; w * h];
    let mut weak = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mg = mag.get(i).copied().unwrap_or(0.0);
            if mg <= lo {
                continue;
            }
            let (ax, ay) = (
                gx.get(i).copied().unwrap_or(0.0).abs(),
                gy.get(i).copied().unwrap_or(0.0).abs(),
            );
            let (xi, yi) = (x as i64, y as i64);
            let tan22 = 0.414_213_562;
            let tan67 = 2.414_213_562;
            let (a, b) = if ay <= ax * tan22 {
                (m(xi - 1, yi), m(xi + 1, yi))
            } else if ay >= ax * tan67 {
                (m(xi, yi - 1), m(xi, yi + 1))
            } else {
                let same_sign =
                    gx.get(i).copied().unwrap_or(0.0) * gy.get(i).copied().unwrap_or(0.0) > 0.0;
                if same_sign {
                    (m(xi - 1, yi - 1), m(xi + 1, yi + 1))
                } else {
                    (m(xi - 1, yi + 1), m(xi + 1, yi - 1))
                }
            };
            if mg > a && mg >= b {
                if mg > hi {
                    if let Some(s) = strong.get_mut(i) {
                        *s = true;
                    }
                } else if let Some(s) = weak.get_mut(i) {
                    *s = true;
                }
            }
        }
    }
    // Isteresi: i deboli collegati a un forte diventano contorno.
    let mut edge = strong.clone();
    let mut stack: Vec<usize> = strong
        .iter()
        .enumerate()
        .filter(|(_, s)| **s)
        .map(|(i, _)| i)
        .collect();
    while let Some(i) = stack.pop() {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        for dy in -1..=1_i64 {
            for dx in -1..=1_i64 {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if weak.get(j).copied().unwrap_or(false) && !edge.get(j).copied().unwrap_or(true) {
                    if let Some(e) = edge.get_mut(j) {
                        *e = true;
                    }
                    stack.push(j);
                }
            }
        }
    }
    edge
}

/// Dilatazione con un quadrato 5x5.
pub fn dilate5(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            if !mask.get(y * w + x).copied().unwrap_or(false) {
                continue;
            }
            for yy in y.saturating_sub(2)..=(y + 2).min(h.saturating_sub(1)) {
                for xx in x.saturating_sub(2)..=(x + 2).min(w.saturating_sub(1)) {
                    if let Some(o) = out.get_mut(yy * w + xx) {
                        *o = true;
                    }
                }
            }
        }
    }
    out
}

/// Modulo della FFT 2D, con lo zero al centro (come `fftshift`). Risultato `h` righe per `w` colonne.
pub fn fft_magnitude_shifted(g: &Gray) -> Vec<f64> {
    let (w, h) = (g.w, g.h);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let mut planner = FftPlanner::<f64>::new();
    let row_fft = planner.plan_fft_forward(w);
    let col_fft = planner.plan_fft_forward(h);
    let mut buf: Vec<Complex<f64>> =
        g.d.iter()
            .map(|v| Complex::new(f64::from(*v), 0.0))
            .collect();
    for row in buf.chunks_exact_mut(w) {
        row_fft.process(row);
    }
    let mut col = vec![Complex::new(0.0, 0.0); h];
    for x in 0..w {
        for (y, c) in col.iter_mut().enumerate() {
            *c = buf.get(y * w + x).copied().unwrap_or_default();
        }
        col_fft.process(&mut col);
        for (y, c) in col.iter().enumerate() {
            if let Some(b) = buf.get_mut(y * w + x) {
                *b = *c;
            }
        }
    }
    let (ky, kx) = (h / 2, w / 2);
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let sy = (y + h - ky) % h;
            let sx = (x + w - kx) % w;
            if let (Some(o), Some(b)) = (out.get_mut(y * w + x), buf.get(sy * w + sx)) {
                *o = b.norm();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, v: u8) -> Gray {
        Gray::new(w, h, vec![v; w * h]).unwrap()
    }

    fn vertical_edge(w: usize, h: usize) -> Gray {
        let mut d = vec![0u8; w * h];
        for y in 0..h {
            for x in w / 2..w {
                d[y * w + x] = 255;
            }
        }
        Gray::new(w, h, d).unwrap()
    }

    #[test]
    fn constructors_reject_wrong_sizes() {
        assert!(Gray::new(0, 4, vec![]).is_none());
        assert!(Gray::new(2, 2, vec![0; 3]).is_none());
        assert!(Gray::new(usize::MAX, 2, vec![]).is_none());
        assert!(Rgb::new(2, 2, vec![0; 11]).is_none());
        assert!(Rgb::new(2, 2, vec![0; 12]).is_some());
    }

    #[test]
    fn gray_matches_opencv_weights() {
        let rgb = Rgb::new(1, 1, vec![255, 0, 0]).unwrap();
        assert_eq!(rgb.to_gray().d, vec![76]);
        let rgb = Rgb::new(1, 1, vec![0, 255, 0]).unwrap();
        assert_eq!(rgb.to_gray().d, vec![150]);
    }

    #[test]
    fn mean_and_std_of_two_levels() {
        let g = Gray::new(2, 1, vec![0, 100]).unwrap();
        assert!((g.mean() - 50.0).abs() < 1e-9);
        assert!((g.std() - 50.0).abs() < 1e-9);
        assert_eq!(
            Gray {
                w: 0,
                h: 0,
                d: vec![]
            }
            .mean(),
            0.0
        );
    }

    #[test]
    fn crop_margin_and_center() {
        let g = Gray::new(4, 4, (0..16).collect()).unwrap();
        let c = g.crop_margin(1).unwrap();
        assert_eq!((c.w, c.h), (2, 2));
        assert_eq!(c.d, vec![5, 6, 9, 10]);
        assert!(g.crop_margin(2).is_none());
        assert!(g.crop_margin(100).is_none());
        let m = g.crop_center(2, 2).unwrap();
        assert_eq!(m.d, vec![5, 6, 9, 10]);
        // Un ritaglio più grande dell'immagine dà l'immagine intera.
        assert_eq!(g.crop_center(99, 99).unwrap(), g);
    }

    #[test]
    fn area_resize_averages_blocks() {
        let g = Gray::new(2, 2, vec![0, 100, 100, 200]).unwrap();
        let r = g.resize_area(1, 1).unwrap();
        assert_eq!(r.d, vec![100]);
        assert!(g.resize_area(0, 1).is_none());
        let h = flat(5, 3, 9).half().unwrap();
        assert_eq!((h.w, h.h), (2, 1));
        assert!(h.d.iter().all(|v| *v == 9));
    }

    #[test]
    fn cubic_upscale_keeps_flat_images_flat_and_size() {
        let r = flat(8, 8, 123).resize_cubic(16, 16).unwrap();
        assert_eq!((r.w, r.h), (16, 16));
        assert!(r.d.iter().all(|v| *v == 123));
        assert!(flat(8, 8, 1).resize_cubic(0, 4).is_none());
    }

    #[test]
    fn pearson_basic_cases() {
        let a = [1.0, 2.0, 3.0, 4.0];
        assert!((pearson(&a, &a) - 1.0).abs() < 1e-12);
        assert!((pearson(&a, &[4.0, 3.0, 2.0, 1.0]) + 1.0).abs() < 1e-12);
        assert_eq!(pearson(&a, &[5.0; 4]), 0.0, "costante: niente NaN");
        assert_eq!(pearson(&a, &[1.0]), 0.0);
        assert_eq!(pearson(&[], &[]), 0.0);
    }

    #[test]
    fn sobel_finds_a_vertical_edge() {
        let (gx, gy) = sobel(&vertical_edge(8, 8));
        // Sul bordo (x=3 e x=4) il gradiente orizzontale è forte, quello verticale nullo.
        assert!(gx[4 * 8 + 3] > 500.0 && gx[4 * 8 + 4] > 500.0);
        assert!(gy.iter().all(|v| v.abs() < 1e-9));
        assert_eq!(gx[4 * 8], 0.0);
        // Immagini minuscole non vanno in panico.
        let _ = sobel(&flat(1, 1, 5));
    }

    #[test]
    fn canny_marks_the_edge_and_ignores_flat_areas() {
        let e = canny(&vertical_edge(32, 32), 50.0, 150.0);
        let count = e.iter().filter(|v| **v).count();
        assert!((32..=96).contains(&count), "contorni: {count}");
        assert!(canny(&flat(32, 32, 200), 50.0, 150.0).iter().all(|v| !*v));
    }

    #[test]
    fn dilate_grows_by_two_pixels() {
        let mut m = vec![false; 11 * 11];
        m[5 * 11 + 5] = true;
        let d = dilate5(&m, 11, 11);
        assert_eq!(d.iter().filter(|v| **v).count(), 25);
        assert!(d[3 * 11 + 3] && !d[2 * 11 + 2]);
    }

    #[test]
    fn fft_of_a_flat_image_has_only_the_center() {
        let m = fft_magnitude_shifted(&flat(16, 16, 10));
        let c = 8 * 16 + 8;
        assert!((m[c] - 2560.0).abs() < 1e-6);
        let rest: f64 = m
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != c)
            .map(|(_, v)| *v)
            .sum();
        assert!(rest < 1e-6);
        assert!(fft_magnitude_shifted(&Gray {
            w: 0,
            h: 0,
            d: vec![]
        })
        .is_empty());
    }

    #[test]
    fn fft_works_for_odd_sizes() {
        let g = Gray::new(5, 3, (0..15).map(|v| (v * 13) as u8).collect()).unwrap();
        let m = fft_magnitude_shifted(&g);
        assert_eq!(m.len(), 15);
        assert!(m.iter().all(|v| v.is_finite()));
    }
}

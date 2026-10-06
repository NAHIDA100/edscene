//! The one piece of image maths the backdrop needs, shared by both platforms
//! so the Windows screen grab and the Linux wallpaper file look the same.

/// Channel order of the source buffer. GDI hands back BGRA; a decoded PNG
/// hands back RGBA. Output is always RGB, so callers must say which they pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Bgra,
    Rgba,
}

impl Order {
    const fn offsets(self) -> (usize, usize, usize) {
        match self {
            Order::Bgra => (2, 1, 0),
            Order::Rgba => (0, 1, 2),
        }
    }
}

/// Average `step x step` blocks and drop the alpha channel, which is also the
/// first third of the blur: a 1/8 sample stretched back over the window reads
/// as a soft backdrop at ~1.5 % of the pixel cost.
pub fn downsample(
    src: &[u8],
    w: usize,
    h: usize,
    step: usize,
    order: Order,
) -> (Vec<u8>, usize, usize) {
    let step = step.max(1);
    let (ro, go, bo) = order.offsets();
    let ow = (w / step).max(1);
    let oh = (h / step).max(1);
    let mut out = vec![0u8; ow * oh * 3];
    for oy in 0..oh {
        for ox in 0..ow {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for dy in 0..step {
                for dx in 0..step {
                    let x = (ox * step + dx).min(w.saturating_sub(1));
                    let y = (oy * step + dy).min(h.saturating_sub(1));
                    let i = (y * w + x) * 4;
                    if i + 3 >= src.len() {
                        continue;
                    }
                    r += src[i + ro] as u32;
                    g += src[i + go] as u32;
                    b += src[i + bo] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            let o = (oy * ow + ox) * 3;
            out[o] = (r / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (b / n) as u8;
        }
    }
    (out, ow, oh)
}

/// A 3x3 box pass over an RGB buffer, in place-ish. Removes the blocky
/// sampling steps so the stretched result looks like a defocus, not a mosaic.
pub fn box_pass_rgb(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = src.to_vec();
    if w < 3 || h < 3 {
        return out;
    }
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            for c in 0..3 {
                let mut acc = 0u32;
                for dy in 0..3 {
                    for dx in 0..3 {
                        acc += src[((y + dy - 1) * w + (x + dx - 1)) * 3 + c] as u32;
                    }
                }
                out[(y * w + x) * 3 + c] = (acc / 9) as u8;
            }
        }
    }
    out
}

/// Downsample + two box passes: the whole backdrop pipeline in one call.
pub fn soften(
    src: &[u8],
    w: usize,
    h: usize,
    step: usize,
    order: Order,
) -> (Vec<u8>, usize, usize) {
    let (a, ow, oh) = downsample(src, w, h, step, order);
    let b = box_pass_rgb(&a, ow, oh);
    let c = box_pass_rgb(&b, ow, oh);
    (c, ow, oh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, rgb: [u8; 3], order: Order) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            let px = match order {
                Order::Bgra => [rgb[2], rgb[1], rgb[0], 255],
                Order::Rgba => [rgb[0], rgb[1], rgb[2], 255],
            };
            v.extend_from_slice(&px);
        }
        v
    }

    #[test]
    fn both_channel_orders_land_on_the_same_rgb() {
        let bgra = flat(16, 16, [10, 20, 30], Order::Bgra);
        let rgba = flat(16, 16, [10, 20, 30], Order::Rgba);
        let (a, _, _) = soften(&bgra, 16, 16, 4, Order::Bgra);
        let (b, _, _) = soften(&rgba, 16, 16, 4, Order::Rgba);
        assert_eq!(a, b);
        assert!(a.chunks(3).all(|p| p == [10, 20, 30]), "{:?}", &a[..3]);
    }

    #[test]
    fn a_hard_edge_is_blurred() {
        // Left half white, right half black, in RGBA order.
        let mut src = Vec::new();
        for _y in 0..8 {
            for x in 0..8 {
                let v = if x < 4 { 255 } else { 0 };
                src.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let (out, w, _) = downsample(&src, 8, 8, 1, Order::Rgba);
        assert_eq!(out[0], 255, "far left stays white");
        assert_eq!(out[(w - 1) * 3], 0, "far right stays black");
        let (soft, sw, sh) = soften(&src, 8, 8, 1, Order::Rgba);
        let mid = soft[((sh / 2) * sw + (sw / 2)) * 3];
        assert!(mid > 40 && mid < 215, "the boundary must soften, got {mid}");
    }

    #[test]
    fn odd_and_empty_sizes_do_not_panic() {
        let src = flat(7, 5, [1, 2, 3], Order::Rgba);
        let (out, w, h) = soften(&src, 7, 5, 8, Order::Rgba);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out.len(), 3);
        let (empty, ew, eh) = downsample(&[], 0, 0, 4, Order::Rgba);
        assert_eq!((ew, eh), (1, 1));
        assert_eq!(empty.len(), 3, "a 1x1 block of nothing reads as black");
        assert_eq!(box_pass_rgb(&[0u8; 3], 1, 1).len(), 3);
    }
}

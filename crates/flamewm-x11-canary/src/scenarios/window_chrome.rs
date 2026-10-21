//! T09/T10 chrome vs client visual probes: checkerboard sampling of the
//! frame (chrome) and the client area; both must deliver and the chrome
//! band must differ from the client mean (visual separation proof).

/// Checkerboard probe over a rect: samples NxN cells, counts delivered and
/// non-black, returns mean of cell means.
fn checkerboard(
    canary: &crate::Canary,
    wid: u32,
    ox: i16,
    oy: i16,
    w: u16,
    h: u16,
    cells: u16,
) -> (u32, u32, u8, u8, u8) {
    let cells = cells.clamp(2, 8);
    let cw = (w / cells).max(1);
    let ch = (h / cells).max(1);
    let (mut delivered, mut non_black, mut n) = (0, 0, 0u64);
    let mut sum = [0u64; 3];
    for cy in 0..cells {
        for cx in 0..cells {
            if (cx + cy) % 2 != 0 {
                continue; // checkerboard: alternate cells only.
            }
            let x = ox.saturating_add((cx * cw) as i16);
            let y = oy.saturating_add((ch * cy) as i16);
            let Ok(d) = canary.sample(wid, x, y, cw.min(32), ch.min(32)) else {
                continue;
            };
            delivered += 1;
            let (az, _, r, g, b) = crate::Canary::pixel_stats(&d);
            non_black += u32::from(!az);
            for (s, v) in sum.iter_mut().zip([r, g, b]) {
                *s += u64::from(v);
            }
            n += 1;
        }
    }
    let nn = n.max(1);
    (
        delivered,
        non_black,
        (sum[0] / nn) as u8,
        (sum[1] / nn) as u8,
        (sum[2] / nn) as u8,
    )
}

pub fn run_t09_t10(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let target = if let Some(id) = crate::arg(args, "--window") {
        id.parse::<u32>().ok().and_then(|id| canary.by_id(id))
    } else if let Some(name) = crate::arg(args, "--name") {
        canary.find(&name).into_iter().next()
    } else {
        canary.find("flame").into_iter().next()
    };
    let Some(w) = target else {
        return (false, "no target path=real-pointer".to_owned());
    };
    if w.width < 48 || w.height < 64 {
        return (
            false,
            format!(
                "id={} too small {}x{} path=real-pointer",
                w.id, w.width, w.height
            ),
        );
    }
    // Chrome = top 28px band; client = remainder below the band.
    let chrome_h: u16 = 28.min(w.height / 3).max(8);
    let (cd, cnb, cr, cg, cb) = checkerboard(canary, w.id, 0, 0, w.width, chrome_h, 4);
    let client_h = w.height.saturating_sub(chrome_h);
    let (dd, dnb, dr, dg, db) =
        checkerboard(canary, w.id, 0, chrome_h as i16, w.width, client_h, 4);
    let chrome_ok = cd > 0 && cnb >= 1;
    let client_ok = dd > 0 && dnb >= 1;
    let differ = cr != dr || cg != dg || cb != db;
    let pass = chrome_ok && client_ok && differ;
    (
        pass,
        format!(
            "id={} chrome_cells={cd} chrome_nb={cnb} chrome_mean=#{cr:02x}{cg:02x}{cb:02x} client_cells={dd} client_nb={dnb} client_mean=#{dr:02x}{dg:02x}{db:02x} differ={differ} path=real-pointer",
            w.id
        ),
    )
}

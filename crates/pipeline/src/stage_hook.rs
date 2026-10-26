//! Test hook: keep one named stage of a CPU render (its buffers and the per-pixel stage's
//! parameters) so a script can replay what follows it outside LightCraft (`lightcraft-cli render
//! --dump-stage NAME=DIR`; the replays live with the test tools). Off unless a [`Capture`] is
//! installed; then each 8-bit CPU render offers its stage and the largest one is kept (an export is
//! the largest render of a run). Nothing here changes a render.
//!
//! Stages ([`STAGES`]):
//! - `sampled`: the source resampled to the output grid (linear camera-space RGB as decoded, before
//!   white balance), `rgb` (3 × f32 per pixel).
//! - `lin`: after white balance, the colour matrix to linear Rec.2020, spots and noise reduction:
//!   `rgb`.
//! - `finish`: the per-pixel stage's inputs: `img` (linear Rec.2020 before exposure), the planes
//!   `log_l`, `base` and, when the settings use them, `texture_blur`, `clarity_blur`, `dark`; the
//!   scalars of [`crate::finish::FinishParams`]; the tone map's table and chroma; the sRGB table;
//!   and `unsupported`, the features this dump leaves out (a replay of it is only valid when that
//!   list is empty).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_raster::{Plane, Rgb32f};

/// The stages a capture can name.
pub const STAGES: [&str; 3] = ["sampled", "lin", "finish"];

static ON: AtomicBool = AtomicBool::new(false);
static SLOT: Mutex<Option<(String, Option<Dump>)>> = Mutex::new(None);

/// One stage's buffers and values.
#[derive(Clone, Debug, Default)]
pub struct Dump {
    pub stage: String,
    pub width: usize,
    pub height: usize,
    /// name, channels, samples (row-major, interleaved)
    pub planes: Vec<(String, usize, Vec<f32>)>,
    pub scalars: Vec<(String, f64)>,
    /// small tables (tone map, sRGB curve, matrices)
    pub arrays: Vec<(String, Vec<f32>)>,
    /// features the dump does not carry
    pub unsupported: Vec<String>,
}

impl Dump {
    pub fn new(stage: &str, width: usize, height: usize) -> Dump {
        Dump { stage: stage.to_string(), width, height, ..Dump::default() }
    }

    pub fn rgb(mut self, name: &str, img: &Rgb32f) -> Dump {
        self.planes.push((name.to_string(), 3, img.data.iter().flatten().copied().collect()));
        self
    }

    pub fn plane(mut self, name: &str, p: &Plane) -> Dump {
        self.planes.push((name.to_string(), 1, p.data.clone()));
        self
    }
}

/// While alive, renders offer [`Capture`]'s stage; dropping it switches the hook off.
pub struct Capture(());

impl Capture {
    /// Start keeping the largest 8-bit CPU render's `stage` (one of [`STAGES`]).
    pub fn install(stage: &str) -> Capture {
        *SLOT.lock().unwrap_or_else(|e| e.into_inner()) = Some((stage.to_string(), None));
        ON.store(true, Ordering::Relaxed);
        Capture(())
    }

    /// The kept dump, if a render offered one.
    pub fn take(&self) -> Option<Dump> {
        SLOT.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|(_, d)| d.take())
    }

    /// Write the kept dump to `dir` (created): `<stage>.json` and one `<stage>.<name>.f32` per
    /// buffer and table (little-endian f32). Returns the JSON's path.
    pub fn write_largest(&self, dir: &Path) -> Result<PathBuf, String> {
        let d = self.take().ok_or("no CPU render offered that stage (it runs only for 8-bit CPU renders)")?;
        write(&d, dir)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        ON.store(false, Ordering::Relaxed);
        *SLOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Whether a capture wants `stage` (cheap: renders call it before building a [`Dump`]).
pub(crate) fn wants(stage: &str) -> bool {
    ON.load(Ordering::Relaxed) && SLOT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|(s, _)| s == stage)
}

/// Offer a dump: kept when it is larger than the one kept so far.
pub(crate) fn offer(d: Dump) {
    let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((stage, kept)) = slot.as_mut()
        && *stage == d.stage
        && kept.as_ref().is_none_or(|k| k.width * k.height < d.width * d.height)
    {
        *kept = Some(d);
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn json_num(v: f64) -> String {
    if v.is_finite() { format!("{v}") } else { "null".into() }
}

fn write_f32(path: &Path, v: &[f32]) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut w = std::io::BufWriter::new(f);
    for x in v {
        w.write_all(&x.to_le_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    w.flush().map_err(|e| format!("{}: {e}", path.display()))
}

/// Write `d` to `dir` (see [`Capture::write_largest`]).
pub fn write(d: &Dump, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut planes = Vec::new();
    for (name, ch, data) in &d.planes {
        let file = format!("{}.{name}.f32", d.stage);
        write_f32(&dir.join(&file), data)?;
        planes.push(format!("{{\"name\":{},\"channels\":{ch},\"file\":{}}}", json_str(name), json_str(&file)));
    }
    let mut arrays = Vec::new();
    for (name, data) in &d.arrays {
        let file = format!("{}.{name}.f32", d.stage);
        write_f32(&dir.join(&file), data)?;
        arrays.push(format!("{}:{{\"len\":{},\"file\":{}}}", json_str(name), data.len(), json_str(&file)));
    }
    let scalars: Vec<String> = d.scalars.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_num(*v))).collect();
    let unsupported: Vec<String> = d.unsupported.iter().map(|s| json_str(s)).collect();
    let json = format!(
        "{{\n\"stage\":{},\n\"width\":{},\n\"height\":{},\n\"planes\":[{}],\n\"scalars\":{{{}}},\n\"arrays\":{{{}}},\n\"unsupported\":[{}]\n}}\n",
        json_str(&d.stage),
        d.width,
        d.height,
        planes.join(","),
        scalars.join(","),
        arrays.join(","),
        unsupported.join(",")
    );
    let path = dir.join(format!("{}.json", d.stage));
    std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// The `finish` stage of a render: `p` and the parameters `finish` builds from them (`display`,
/// `proof`: the request draws into a display space / soft-proofs; `keep`: the window cut from the
/// result afterwards, `[x, y, w, h]`).
pub(crate) fn finish_dump(p: &crate::Prepared, fp: &crate::finish::FinishParams, display: bool, proof: bool, keep: Option<[usize; 4]>) -> Dump {
    let (w, h) = (p.img.width, p.img.height);
    let mut d = Dump::new("finish", w, h).rgb("img", &p.img).plane("log_l", &p.log_l).plane("base", &p.base);
    for (name, pl) in [("clarity_blur", &p.clarity_blur), ("texture_blur", &p.texture_blur), ("dark", &p.dark)] {
        if let Some(pl) = pl {
            d = d.plane(name, pl);
        }
    }
    let mut un = Vec::new();
    let mut no = |cond: bool, what: &str| {
        if cond {
            un.push(what.to_string());
        }
    };
    no(!p.masks.is_empty(), "masks");
    no(fp.calib.is_some() || fp.shadow_tint != 0.0, "calibration");
    no(fp.curves.is_some(), "tone curves");
    no(fp.vig.is_some(), "vignette");
    no(fp.grain.is_some(), "grain");
    no(fp.lut.is_some(), "LUT profile");
    no(proof || fp.proof.is_some(), "soft proof");
    no(display, "display space");
    no(fp.out_trc != crate::OutputTrc::Srgb, "non-sRGB transfer curve");
    no(!fp.ops.is_identity(), "colour ops (vibrance, saturation, mixer, points, B&W, grading)");
    no(fp.peak != 1.0, "HDR peak");
    d.unsupported = un;
    let k = keep.unwrap_or([0, 0, w, h]);
    d.scalars = vec![
        ("gain".into(), fp.gain as f64),
        ("ev".into(), fp.ev as f64),
        ("hl".into(), fp.hl as f64),
        ("sh".into(), fp.sh as f64),
        ("clar".into(), fp.clar as f64),
        ("tex".into(), fp.tex as f64),
        ("dehaze".into(), fp.dehaze as f64),
        ("sharpen".into(), fp.sharpen as f64),
        ("sharpen_mask".into(), fp.sharpen_mask as f64),
        ("air".into(), fp.air as f64),
        ("air_pre".into(), fp.air_pre as f64),
        ("peak".into(), fp.peak as f64),
        ("grey".into(), crate::tone::GREY as f64),
        ("tone_min_ev".into(), crate::tone::LUT_MIN_EV as f64),
        ("tone_max_ev".into(), crate::tone::LUT_MAX_EV as f64),
        ("keep_x".into(), k[0] as f64),
        ("keep_y".into(), k[1] as f64),
        ("keep_w".into(), k[2] as f64),
        ("keep_h".into(), k[3] as f64),
    ];
    d.arrays = vec![
        ("tone_lut".into(), fp.tone.lut().to_vec()),
        ("tone_chroma".into(), fp.tone.chroma_lut().to_vec()),
        ("srgb_lut".into(), crate::finish::srgb_lut().to_vec()),
        ("to_out".into(), fp.to_out.iter().flatten().copied().collect()),
        ("out_luma".into(), fp.out_luma.to_vec()),
        ("luma_2020".into(), lightcraft_color::LUMA_2020.to_vec()),
    ];
    d
}

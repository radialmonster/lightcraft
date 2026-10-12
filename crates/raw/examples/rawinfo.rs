//! `cargo run --release -p lightcraft-raw --example rawinfo -- [--tree] [--json] [--preview-out DIR [--overwrite]] [--] FILE...`
//!
//! Prints the TIFF structure (with `--tree`), decode result, timings (MP/s) and basic statistics.
//!
//! `--preview-out DIR` writes the largest embedded preview JPEG of each file into `DIR`: the bytes returned by
//! [`embedded_preview`] (which trims padding after the JPEG end marker), with no orientation applied (rotate by
//! `orientation`). `DIR` is created if needed; when it can't be created or written to, nothing is processed (exit
//! status 2). Files are never overwritten: each preview gets the first free name of `<stem>.jpg`,
//! `<stem>.<ext>.jpg` (the input's extension), `<stem>.<ext>-2.jpg`, `-3`, …, so inputs with the same stem
//! (other folders or formats) and files already in `DIR` are kept. `--overwrite` lets a preview replace a file
//! that existed before the run (still one name per input within a run).
//!
//! Exit status: 0 when every file was read and every requested preview written (whatever the decode
//! `status`: an unsupported or corrupt raw is a result, not a failure); 1 when an input could not be read, a
//! decoder panicked or a requested preview could not be written (the other files are still processed); 2 for
//! a usage error (unknown option, `--preview-out` without a directory or followed by an option, no input files,
//! unusable `--preview-out` directory), reported on stderr before any file is processed.
//!
//! `--json` prints one JSON object per file (JSON Lines) instead, without the demosaic timings. Every line has
//! every field below, `null` when unknown or not applicable, including when the file could not be read:
//! - `file`, `file_bytes`; `format`: `"Dng"`, `"Cr2"`, `"Cr3"`, `"Nef"`, `"Nrw"`, `"Arw"`, `"Raf"`, `"Orf"`,
//!   `"Rw2"`, `"Pef"`, `"Srw"`, `"OtherTiff"` or `null` (not a raw file).
//! - `status`: `"ok"` (decoded), `"unsupported"` (not a raw file or a variant not decoded yet), `"error"`
//!   (unreadable or corrupt), `"panic"` (a decoder bug); `error_kind`: the [`RawError`] variant (`"NotRaw"`,
//!   `"Unsupported"`, `"Corrupt"`, `"Limit"`, `"Tiff"`), `"Io"` (the file could not be read), `"Panic"` or
//!   `null`; `reason`: the decoder's (or for `"Io"` the system's) error text, verbatim; `decode_ms`.
//! - `info_source`: `"decode"`, `"header"` (decode failed but the headers were read, [`probe_info`]) or `null`.
//!   When it is `null`, the fields below up to `lens_corrections` are `null`, except the camera/lens/exposure
//!   fields and `orientation`, which then come from the metadata reader (`lightcraft_meta::extract`).
//! - `make`, `model`, `lens_make`, `lens_model`, `iso`, `f_number`, `exposure_time` (s), `focal_length` (mm),
//!   `capture_time` (ISO 8601).
//! - `width`, `height` (sensor data, masked borders included), `cpp`, `cfa` (`"RGGB"`, …; 36 letters for
//!   X-Trans), `bits`, `active_area` and `crop` (`{x, y, width, height}`; the crop is relative to the active
//!   area), `developed_size` (`[w, h]` before orientation), `orientation` (Exif 1–8), `orientation_name`.
//! - `black_mean`, `white` (per plane): decoded files only.
//! - `color`: `has_color_matrix`, `matrix_is_fallback` (no matrix: a generic camera ≈ sRGB model is used),
//!   `illuminants` (Exif light source codes), `as_shot_neutral`, `as_shot_white_xy` (as stored), `wb_multipliers`
//!   (camera multipliers as stored), `as_shot_xy` and `as_shot_wb` (the white point and multipliers LightCraft
//!   develops with), `baseline_exposure` (EV).
//! - `opcodes`: `{list1, list2, list3}`, each a list of `{name, applied, lens_correction, params}`; `params` holds
//!   the coefficients of `WarpRectilinear` (`planes` of kr0..kr3, kt0, kt1; `center`), `WarpRectilinear2`
//!   (`planes` of 19 values, `center`, `reciprocal`, `optional`), `WarpFisheye`,
//!   `FixVignetteRadial` (`k`, `center`), the bounds of `TrimBounds`, the id of an unknown opcode, else `null`.
//!   `lens_corrections`: whether any list has a warp or vignette opcode.
//! - `preview`: always an object `{status, error, path, bytes, width, height}`, independent of the decode
//!   `status`. `status` is `"written"` (to `path`), `"none"` (no embedded JPEG), `"not-requested"` (found, or the
//!   input unreadable, without `--preview-out`) or `"error"` (`error` says why: the write failed, or the input
//!   could not be read). `bytes`, `width`, `height` describe the JPEG whenever one was found.
//! - `tree` (only with `--tree`, then on every line): the TIFF IFD chain, each IFD `{label, offset, entries,
//!   sub_ifds, exif, gps, interop}`, each entry `{tag, type, count, value}` (text, the first 64 values, or the
//!   first 64 bytes as hex); `null` when the file is unreadable or not TIFF-based.

use lightcraft_raw::opcodes::Opcode;
use lightcraft_raw::{Metadata, Method, RawData, RawError, RawInfo, Rect, decode, embedded_preview, probe, probe_info};
use lightcraft_tiff::{Ifd, Tiff, Value};
use serde_json::{Value as Json, json};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn short(v: &Value) -> String {
    let s = match v {
        Value::Ascii(s) => format!("{s:?}"),
        Value::Byte(b) | Value::Undefined(b) => format!("{} bytes {:02x?}", b.len(), &b[..b.len().min(12)]),
        other => {
            let f = other.to_f64_vec();
            format!("{:?}{}", &f[..f.len().min(12)], if f.len() > 12 { format!(" …({})", f.len()) } else { String::new() })
        }
    };
    s.chars().take(160).collect()
}

fn dump(ifd: &Ifd, depth: usize, label: &str) {
    let pad = "  ".repeat(depth);
    println!("{pad}{label} @{} ({} entries)", ifd.offset, ifd.entries.len());
    for e in &ifd.entries {
        println!("{pad}  {:5} 0x{:04x} {:?}: {}", e.tag, e.tag, e.field_type(), short(&e.value));
    }
    for (i, s) in ifd.sub_ifds.iter().enumerate() {
        dump(s, depth + 1, &format!("SubIFD[{i}]"));
    }
    for (n, c) in [("Exif", &ifd.exif), ("GPS", &ifd.gps), ("Interop", &ifd.interop)] {
        if let Some(c) = c {
            dump(c, depth + 1, n);
        }
    }
}

fn ifd_json(ifd: &Ifd, label: &str) -> Json {
    let entries: Vec<Json> = ifd
        .entries
        .iter()
        .map(|e| {
            let value = match &e.value {
                Value::Ascii(s) => json!(s),
                Value::Byte(b) | Value::Undefined(b) => json!(b.iter().take(64).map(|x| format!("{x:02x}")).collect::<String>()),
                other => json!(other.to_f64_vec().into_iter().take(64).collect::<Vec<_>>()),
            };
            json!({"tag": e.tag, "type": format!("{:?}", e.field_type()), "count": e.count(), "value": value})
        })
        .collect();
    let sub = |c: &Option<Box<Ifd>>, n: &str| c.as_ref().map(|c| ifd_json(c, n));
    json!({
        "label": label,
        "offset": ifd.offset,
        "entries": entries,
        "sub_ifds": ifd.sub_ifds.iter().enumerate().map(|(i, s)| ifd_json(s, &format!("SubIFD[{i}]"))).collect::<Vec<_>>(),
        "exif": sub(&ifd.exif, "Exif"),
        "gps": sub(&ifd.gps, "GPS"),
        "interop": sub(&ifd.interop, "Interop"),
    })
}

fn rect(r: Rect) -> Json {
    json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height})
}

fn is_lens_correction(op: &Opcode) -> bool {
    matches!(op, Opcode::WarpRectilinear { .. } | Opcode::WarpRectilinear2 { .. } | Opcode::WarpFisheye { .. } | Opcode::FixVignetteRadial { .. })
}

fn opcode_json(op: &Opcode) -> Json {
    let (name, params) = match op {
        Opcode::WarpRectilinear { planes, center } => ("WarpRectilinear", json!({"planes": planes, "center": center})),
        Opcode::WarpFisheye { planes, center } => ("WarpFisheye", json!({"planes": planes, "center": center})),
        Opcode::WarpRectilinear2 { planes, center, reciprocal, optional } => {
            ("WarpRectilinear2", json!({"planes": planes, "center": center, "reciprocal": reciprocal, "optional": optional}))
        }
        Opcode::FixVignetteRadial { k, center } => ("FixVignetteRadial", json!({"k": k, "center": center})),
        Opcode::FixBadPixelsConstant { .. } => ("FixBadPixelsConstant", Json::Null),
        Opcode::FixBadPixelsList { .. } => ("FixBadPixelsList", Json::Null),
        Opcode::TrimBounds { top, left, bottom, right } => ("TrimBounds", json!({"top": top, "left": left, "bottom": bottom, "right": right})),
        Opcode::MapTable { .. } => ("MapTable", Json::Null),
        Opcode::MapPolynomial { .. } => ("MapPolynomial", Json::Null),
        Opcode::GainMap { .. } => ("GainMap", Json::Null),
        Opcode::DeltaPerRow { .. } => ("DeltaPerRow", Json::Null),
        Opcode::DeltaPerColumn { .. } => ("DeltaPerColumn", Json::Null),
        Opcode::ScalePerRow { .. } => ("ScalePerRow", Json::Null),
        Opcode::ScalePerColumn { .. } => ("ScalePerColumn", Json::Null),
        Opcode::Unknown { id, .. } => ("Unknown", json!({"id": id})),
    };
    json!({"name": name, "applied": op.is_applied(), "lens_correction": is_lens_correction(op), "params": params})
}

/// `(width, height)` from the first SOF marker of a JPEG.
fn jpeg_size(b: &[u8]) -> Option<(u16, u16)> {
    let mut i = 2;
    while b.get(i) == Some(&0xff) {
        let m = *b.get(i + 1)?;
        if m == 0xff {
            i += 1;
            continue;
        }
        let be16 = |at: usize| b.get(at..at + 2).map(|s| u16::from_be_bytes([s[0], s[1]]));
        if matches!(m, 0xc0..=0xcf) && !matches!(m, 0xc4 | 0xc8 | 0xcc) {
            return Some((be16(i + 7)?, be16(i + 5)?));
        }
        i += 2 + be16(i + 2)? as usize;
    }
    None
}

/// Writes previews into one directory, never over a file it didn't write in this run: the first free name of
/// `<stem>.jpg`, `<stem>.<ext>.jpg` (the input's extension), `<stem>.<ext>-2.jpg`, `-3`, …. With `--overwrite`,
/// files that existed before the run may be replaced, but a name is still used once per run (compared
/// case-insensitively, for case-insensitive file systems).
struct PreviewWriter {
    dir: PathBuf,
    overwrite: bool,
    used: HashSet<String>,
}

impl PreviewWriter {
    fn new(dir: &Path, overwrite: bool) -> PreviewWriter {
        PreviewWriter { dir: dir.to_path_buf(), overwrite, used: HashSet::new() }
    }

    /// The `n`-th candidate file name for `input`.
    fn candidate(input: &str, n: usize) -> String {
        let p = Path::new(input);
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "preview".into());
        let base = match p.extension() {
            Some(e) => format!("{stem}.{}", e.to_string_lossy()),
            None => stem.clone(),
        };
        match n {
            0 => format!("{stem}.jpg"),
            1 => format!("{base}.jpg"),
            n => format!("{base}-{n}.jpg"),
        }
    }

    fn write(&mut self, input: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        for n in 0..10_000 {
            let name = Self::candidate(input, n);
            if self.used.contains(&name.to_lowercase()) {
                continue;
            }
            let path = self.dir.join(&name);
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true);
            if self.overwrite {
                opts.create(true).truncate(true);
            } else {
                opts.create_new(true);
            }
            match opts.open(&path) {
                Ok(mut f) => {
                    self.used.insert(name.to_lowercase());
                    return match f.write_all(bytes).and_then(|()| f.sync_all()) {
                        Ok(()) => Ok(path),
                        Err(e) => {
                            drop(f);
                            let _ = std::fs::remove_file(&path);
                            Err(format!("{}: {e}", path.display()))
                        }
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Err(format!("{}: no free file name for {input}", self.dir.display()))
    }
}

/// The `preview` object (see the module docs) and whether a requested write failed. `bytes` is `None` when
/// the input could not be read.
fn preview_json(bytes: Option<&[u8]>, input: &str, writer: Option<&mut PreviewWriter>) -> (Json, bool) {
    let mut o = json!({"status": "none", "error": null, "path": null, "bytes": null, "width": null, "height": null});
    let Some(bytes) = bytes else {
        if writer.is_some() {
            o["status"] = json!("error");
            o["error"] = json!("the input file could not be read");
            return (o, true);
        }
        o["status"] = json!("not-requested");
        return (o, false);
    };
    let jpeg = match std::panic::catch_unwind(|| embedded_preview(bytes)) {
        Ok(Some(j)) => j,
        Ok(None) => return (o, false),
        Err(_) => {
            o["status"] = json!("error");
            o["error"] = json!("embedded_preview panicked");
            return (o, true);
        }
    };
    let wh = jpeg_size(&jpeg);
    o["bytes"] = json!(jpeg.len());
    o["width"] = json!(wh.map(|s| s.0));
    o["height"] = json!(wh.map(|s| s.1));
    let Some(w) = writer else {
        o["status"] = json!("not-requested");
        return (o, false);
    };
    match w.write(input, &jpeg) {
        Ok(path) => {
            o["status"] = json!("written");
            o["path"] = json!(path.display().to_string());
            (o, false)
        }
        Err(e) => {
            o["status"] = json!("error");
            o["error"] = json!(e);
            (o, true)
        }
    }
}

fn meta_json(m: &Metadata, out: &mut serde_json::Map<String, Json>) {
    let fields = json!({
        "make": m.make, "model": m.model, "lens_make": m.lens_make, "lens_model": m.lens_model,
        "iso": m.iso, "f_number": m.f_number, "exposure_time": m.exposure_time, "focal_length": m.focal_length,
        "capture_time": m.capture_time.map(|d| d.to_iso()),
    });
    if let Json::Object(f) = fields {
        out.extend(f);
    }
}

fn info_json(i: &RawInfo, out: &mut serde_json::Map<String, Json>) {
    let m = &i.metadata;
    let c = &i.color;
    let xy = lightcraft_raw::color::as_shot_white_xy_of(i);
    let has_matrix = lightcraft_raw::color::has_matrix(c);
    let lists = [&i.opcodes.list1, &i.opcodes.list2, &i.opcodes.list3];
    let lens = lists.iter().flat_map(|l| l.iter()).any(is_lens_correction);
    meta_json(m, out);
    let fields = json!({
        "width": i.width, "height": i.height, "cpp": i.cpp, "cfa": i.cfa.as_ref().map(|c| c.name()), "bits": i.bits,
        "active_area": rect(i.active_area), "crop": rect(i.crop), "developed_size": i.developed_size(),
        "orientation": i.orientation.to_exif(), "orientation_name": format!("{:?}", i.orientation),
        "color": {
            "has_color_matrix": has_matrix,
            "matrix_is_fallback": !has_matrix,
            "illuminants": c.illuminant,
            "as_shot_neutral": c.as_shot_neutral,
            "as_shot_white_xy": c.as_shot_white_xy.map(|w| [w.x, w.y]),
            "wb_multipliers": i.wb_multipliers,
            "as_shot_xy": [xy.x, xy.y],
            "as_shot_wb": lightcraft_raw::color::wb_multipliers(c, xy),
            "baseline_exposure": c.baseline_exposure,
        },
        "opcodes": {
            "list1": i.opcodes.list1.iter().map(opcode_json).collect::<Vec<_>>(),
            "list2": i.opcodes.list2.iter().map(opcode_json).collect::<Vec<_>>(),
            "list3": i.opcodes.list3.iter().map(opcode_json).collect::<Vec<_>>(),
        },
        "lens_corrections": lens,
    });
    if let Json::Object(f) = fields {
        out.extend(f);
    }
}

/// Every key of a `--json` line (`tree` is added with `--tree`).
const KEYS: [&str; 33] = [
    "file",
    "file_bytes",
    "format",
    "status",
    "error_kind",
    "reason",
    "decode_ms",
    "info_source",
    "make",
    "model",
    "lens_make",
    "lens_model",
    "iso",
    "f_number",
    "exposure_time",
    "focal_length",
    "capture_time",
    "width",
    "height",
    "cpp",
    "cfa",
    "bits",
    "active_area",
    "crop",
    "developed_size",
    "orientation",
    "orientation_name",
    "black_mean",
    "white",
    "color",
    "opcodes",
    "lens_corrections",
    "preview",
];

/// One `--json` record and whether any of this file's outputs failed (see the exit status in the module docs).
fn json_line(path: &str, tree: bool, writer: Option<&mut PreviewWriter>) -> (Json, bool) {
    let mut o = serde_json::Map::new();
    for k in KEYS {
        o.insert(k.into(), Json::Null);
    }
    if tree {
        o.insert("tree".into(), Json::Null);
    }
    o.insert("file".into(), json!(path));
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            o.insert("status".into(), json!("error"));
            o.insert("error_kind".into(), json!("Io"));
            o.insert("reason".into(), json!(e.to_string()));
            o.insert("preview".into(), preview_json(None, path, writer).0);
            return (Json::Object(o), true);
        }
    };
    o.insert("file_bytes".into(), json!(bytes.len()));
    o.insert("format".into(), json!(probe(&bytes).map(|f| format!("{f:?}"))));
    let t0 = Instant::now();
    let decoded = std::panic::catch_unwind(|| decode(&bytes));
    o.insert("decode_ms".into(), json!(t0.elapsed().as_secs_f64() * 1e3));
    let (status, reason, info) = match decoded {
        Ok(Ok(r)) => {
            o.insert("black_mean".into(), json!(r.black.mean()));
            o.insert("white".into(), json!(r.white));
            ("ok", None, Some((r.into_info(), "decode")))
        }
        Ok(Err(e)) => {
            let (status, kind) = match e {
                RawError::NotRaw => ("unsupported", "NotRaw"),
                RawError::Unsupported(_) => ("unsupported", "Unsupported"),
                RawError::Corrupt(_) => ("error", "Corrupt"),
                RawError::Limit(_) => ("error", "Limit"),
                RawError::Tiff(_) => ("error", "Tiff"),
            };
            o.insert("error_kind".into(), json!(kind));
            let header = std::panic::catch_unwind(|| probe_info(&bytes)).ok().and_then(|r| r.ok());
            (status, Some(e.to_string()), header.map(|i| (i, "header")))
        }
        Err(_) => {
            o.insert("error_kind".into(), json!("Panic"));
            ("panic", Some("decoder panicked".to_string()), None)
        }
    };
    o.insert("status".into(), json!(status));
    o.insert("reason".into(), json!(reason));
    o.insert("info_source".into(), json!(info.as_ref().map(|(_, s)| *s)));
    match &info {
        Some((i, _)) => info_json(i, &mut o),
        None if status != "panic" => {
            // what the metadata reader finds (e.g. CR3 and other undecoded variants)
            let m = lightcraft_meta::extract(&bytes);
            meta_json(&m, &mut o);
            o.insert("orientation".into(), json!(m.orientation.map(|r| r.to_exif())));
        }
        None => {}
    }
    let (preview, preview_failed) = preview_json(Some(&bytes), path, writer);
    o.insert("preview".into(), preview);
    if tree {
        let t = Tiff::parse(&bytes).ok().map(|t| t.ifds.iter().enumerate().map(|(i, ifd)| ifd_json(ifd, &format!("IFD{i}"))).collect::<Vec<_>>());
        o.insert("tree".into(), json!(t));
    }
    (Json::Object(o), status == "panic" || preview_failed)
}

const USAGE: &str = "usage: rawinfo [--tree] [--json] [--preview-out DIR [--overwrite]] [--] FILE...";

/// Command-line options.
#[derive(Debug, Default, PartialEq)]
struct Opts {
    tree: bool,
    json: bool,
    overwrite: bool,
    preview_out: Option<PathBuf>,
    files: Vec<String>,
}

fn parse_args(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut only_files = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            _ if only_files => o.files.push(a.clone()),
            "--" => only_files = true,
            "--tree" => o.tree = true,
            "--json" => o.json = true,
            "--overwrite" => o.overwrite = true,
            "--preview-out" => match it.next() {
                _ if o.preview_out.is_some() => return Err("--preview-out given twice".into()),
                Some(v) if v.starts_with("--") => return Err(format!("--preview-out needs a directory, not the option `{v}`")),
                Some(v) if !v.is_empty() => o.preview_out = Some(v.into()),
                _ => return Err("--preview-out needs a directory".into()),
            },
            _ if a.starts_with("--") => return Err(format!("unknown option `{a}`")),
            _ => o.files.push(a.clone()),
        }
    }
    if o.overwrite && o.preview_out.is_none() {
        return Err("--overwrite only applies with --preview-out".into());
    }
    if o.files.is_empty() {
        return Err("no input files".into());
    }
    Ok(o)
}

/// Create the `--preview-out` directory if needed and check that files can be created in it.
fn prepare_out_dir(dir: &Path) -> Result<(), String> {
    if dir.exists() && !dir.is_dir() {
        return Err(format!("--preview-out {}: exists and is not a directory", dir.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("--preview-out {}: cannot create the directory: {e}", dir.display()))?;
    let probe = dir.join(format!(".rawinfo-write-test-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| format!("--preview-out {}: cannot create files there: {e}", dir.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&args));
}

/// Runs the command; returns the exit status (see the module docs).
fn run(args: &[String]) -> i32 {
    let opts = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("rawinfo: {e}\n{USAGE}");
            return 2;
        }
    };
    let mut writer = match &opts.preview_out {
        Some(dir) => match prepare_out_dir(dir) {
            Ok(()) => Some(PreviewWriter::new(dir, opts.overwrite)),
            Err(e) => {
                eprintln!("rawinfo: {e}\n{USAGE}");
                return 2;
            }
        },
        None => None,
    };
    let mut failed = false;
    for path in &opts.files {
        failed |= if opts.json {
            let (line, f) = json_line(path, opts.tree, writer.as_mut());
            println!("{line}");
            f
        } else {
            human(path, opts.tree, writer.as_mut())
        };
    }
    i32::from(failed)
}

/// The human-readable report of one file; returns whether any of its outputs failed.
fn human(path: &str, tree: bool, writer: Option<&mut PreviewWriter>) -> bool {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            println!("== {path}: cannot read: {e}");
            let (p, _) = preview_json(None, path, writer);
            if let Some(e) = p["error"].as_str() {
                println!("  preview: {e}");
            }
            return true;
        }
    };
    println!("== {path} ({:.1} MB) format {:?}", bytes.len() as f64 / 1e6, probe(&bytes));
    if tree && let Ok(t) = Tiff::parse(&bytes) {
        for (i, ifd) in t.ifds.iter().enumerate() {
            dump(ifd, 0, &format!("IFD{i}"));
        }
    }
    let t0 = Instant::now();
    match decode(&bytes) {
        Ok(r) => {
            let dt = t0.elapsed().as_secs_f64();
            let mp = (r.width * r.height) as f64 / 1e6;
            let (mn, mx, mean) = match &r.data {
                RawData::U16(v) => {
                    let mn = *v.iter().min().unwrap_or(&0) as f64;
                    let mx = *v.iter().max().unwrap_or(&0) as f64;
                    (mn, mx, v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64)
                }
                RawData::F32(v) => (
                    v.iter().cloned().fold(f32::MAX, f32::min) as f64,
                    v.iter().cloned().fold(f32::MIN, f32::max) as f64,
                    v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64,
                ),
            };
            println!(
                "  {}x{} cpp {} bits {} cfa {:?} black {:.1} white {:?} active {:?} crop {:?} orient {:?}",
                r.width,
                r.height,
                r.cpp,
                r.bits,
                r.cfa.as_ref().map(|c| c.name()),
                r.black.mean(),
                r.white,
                r.active_area,
                r.crop,
                r.orientation
            );
            println!(
                "  data min {mn} max {mx} mean {mean:.1}; wb {:?}; opcodes {}/{}/{}",
                r.wb_multipliers,
                r.opcodes.list1.len(),
                r.opcodes.list2.len(),
                r.opcodes.list3.len()
            );
            println!("  decode {:.0} ms ({:.0} MP/s)", dt * 1e3, mp / dt);
            println!(
                "  meta: {:?} {:?} lens {:?} iso {:?} f {:?} t {:?} date {:?}",
                r.metadata.make,
                r.metadata.model,
                r.metadata.lens_model,
                r.metadata.iso,
                r.metadata.f_number,
                r.metadata.exposure_display(),
                r.metadata.capture_time.map(|d| d.to_iso())
            );
            let xy = lightcraft_raw::color::as_shot_white_xy(&r);
            let ct = lightcraft_raw::color::camera_transform(&r, xy);
            println!("  as-shot xy ({:.4}, {:.4}) wb {:?} fallback {}", xy.x, xy.y, ct.wb, ct.matrix_is_fallback);
            for m in [Method::Bilinear, Method::Ppg, Method::Ahd] {
                let t1 = Instant::now();
                match r.develop(m) {
                    Ok(img) => {
                        let dt = t1.elapsed().as_secs_f64();
                        let mut s = [0f64; 3];
                        for p in &img.data {
                            for c in 0..3 {
                                s[c] += p[c] as f64;
                            }
                        }
                        let n = img.data.len() as f64;
                        println!(
                            "  develop {m:?}: {}x{} in {:.0} ms ({:.0} MP/s), mean rgb {:.3} {:.3} {:.3}",
                            img.width,
                            img.height,
                            dt * 1e3,
                            mp / dt,
                            s[0] / n,
                            s[1] / n,
                            s[2] / n
                        );
                    }
                    Err(e) => println!("  develop {m:?}: {e}"),
                }
            }
        }
        Err(e) => println!("  decode error: {e}"),
    }
    let (p, failed) = preview_json(Some(&bytes), path, writer);
    match (p["status"].as_str(), p["bytes"].as_u64()) {
        (_, None) => println!("  preview: none"),
        (Some("written"), Some(n)) => println!("  preview: {n} bytes {}x{} -> {}", p["width"], p["height"], p["path"].as_str().unwrap_or("")),
        (Some("error"), Some(n)) => println!("  preview: {n} bytes, not written: {}", p["error"].as_str().unwrap_or("")),
        (_, Some(n)) => println!("  preview: {n} bytes {}x{}", p["width"], p["height"]),
    }
    failed
}

// Run by `cargo test` (`test = true` for this example in Cargo.toml).
#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_raw::{BlackLevel, Cfa, ColorData, DngWriteOptions, Mat3, OpcodeLists, Orientation, RawFormat, RawImage, write_dng};
    use lightcraft_tiff::{IfdBuilder, ImageData, TiffWriter, tags as t};

    /// A fresh, empty scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rawinfo-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|a| a.to_string()).collect()
    }

    fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let px = vec![90u8; w as usize * h as usize * 3];
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, 80).encode(&px, w, h, jpeg_encoder::ColorType::Rgb).unwrap();
        out
    }

    /// A plain TIFF (not a raw) whose IFD0 is a JPEG strip: `embedded_preview` finds it.
    fn tiff_with_preview(j: &[u8]) -> Vec<u8> {
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(t::COMPRESSION, Value::Short(vec![6]));
        ifd0.set(t::IMAGE_WIDTH, Value::Long(vec![12]));
        ifd0.set(t::IMAGE_LENGTH, Value::Long(vec![8]));
        ifd0.set_image(ImageData::Strips { rows_per_strip: 8, strips: vec![j.to_vec()] });
        TiffWriter::default().write(&[ifd0]).unwrap()
    }

    /// A small DNG written by the crate's own writer, rotated 90° (Exif orientation 6).
    fn rotated_dng() -> Vec<u8> {
        let (w, h) = (16, 12);
        let raw = RawImage {
            format: RawFormat::Dng,
            width: w,
            height: h,
            cpp: 1,
            data: RawData::U16((0..w * h).map(|i| 300 + (i % 97) as u16 * 50).collect()),
            cfa: Cfa::bayer("RGGB"),
            bits: 14,
            black: BlackLevel::uniform(256.0),
            white: vec![16000.0],
            active_area: Rect::new(0, 0, w, h),
            crop: Rect::new(0, 0, w, h),
            orientation: Orientation::Rotate90,
            color: ColorData {
                illuminant: [21, 0],
                color_matrix: [Some(Mat3([[0.9, 0.2, -0.15], [-0.3, 1.25, 0.08], [0.02, -0.12, 0.85]])), None],
                as_shot_neutral: Some([0.5, 1.0, 0.7]),
                ..Default::default()
            },
            wb_multipliers: None,
            linearized: false,
            opcodes: OpcodeLists::default(),
            metadata: Metadata { make: Some("Synth".into()), model: Some("Cam 1".into()), ..Default::default() },
        };
        write_dng(&raw, &DngWriteOptions::default()).unwrap()
    }

    fn assert_every_key(line: &Json, tree: bool) {
        let o = line.as_object().unwrap();
        let mut want: Vec<&str> = KEYS.to_vec();
        if tree {
            want.push("tree");
        }
        let mut got: Vec<&str> = o.keys().map(String::as_str).collect();
        want.sort_unstable();
        got.sort_unstable();
        assert_eq!(got, want);
        let p = o["preview"].as_object().unwrap();
        let mut pk: Vec<&str> = p.keys().map(String::as_str).collect();
        pk.sort_unstable();
        assert_eq!(pk, ["bytes", "error", "height", "path", "status", "width"]);
    }

    #[test]
    fn arguments() {
        let o = parse_args(&s(&["--tree", "--json", "--preview-out", "out", "--overwrite", "a.arw", "--", "--b.nef"])).unwrap();
        assert!(o.tree && o.json && o.overwrite);
        assert_eq!(o.preview_out, Some(PathBuf::from("out")));
        assert_eq!(o.files, ["a.arw", "--b.nef"]);
        for bad in [
            &["--preview-out"][..],
            &["a.arw", "--preview-out"],
            &["--preview-out", "--json", "a.arw"],
            &["--preview-out", "", "a.arw"],
            &["--preview-out", "x", "--preview-out", "y", "a.arw"],
            &["--overwrite", "a.arw"],
            &["--jsn", "a.arw"],
            &["--json"],
            &[],
        ] {
            assert!(parse_args(&s(bad)).is_err(), "{bad:?}");
            assert_eq!(run(&s(bad)), 2, "{bad:?}");
        }
    }

    #[test]
    fn invalid_destination_is_a_usage_error() {
        let d = scratch("dest");
        let file = d.join("file.txt");
        std::fs::write(&file, b"x").unwrap();
        let input = d.join("in.tif");
        std::fs::write(&input, tiff_with_preview(&jpeg(12, 8))).unwrap();
        assert!(prepare_out_dir(&file).unwrap_err().contains("not a directory"));
        assert!(prepare_out_dir(&file.join("sub")).is_err());
        let args = |dest: &Path| s(&["--json", "--preview-out", &dest.to_string_lossy(), &input.to_string_lossy()]);
        assert_eq!(run(&args(&file)), 2);
        assert_eq!(run(&args(&file.join("sub"))), 2);
        assert_eq!(std::fs::read(&file).unwrap(), b"x");
        // a new nested directory is created and the run succeeds
        assert_eq!(run(&args(&d.join("new/nested"))), 0);
        assert!(d.join("new/nested/in.jpg").is_file());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn duplicate_stems_and_existing_files_are_kept() {
        let d = scratch("names");
        std::fs::write(d.join("x.jpg"), b"keep").unwrap();
        let mut w = PreviewWriter::new(&d, false);
        let names: Vec<String> = ["a/x.arw", "b/x.arw", "c/x.nef", "d/y.dng", "e/X.ARW"]
            .iter()
            .map(|i| w.write(i, i.as_bytes()).unwrap().file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["x.arw.jpg", "x.arw-2.jpg", "x.nef.jpg", "y.jpg", "X.ARW-3.jpg"]);
        assert_eq!(std::fs::read(d.join("x.jpg")).unwrap(), b"keep");
        assert_eq!(std::fs::read(d.join("x.arw-2.jpg")).unwrap(), b"b/x.arw");
        // a second run without --overwrite keeps the first run's files
        let mut w2 = PreviewWriter::new(&d, false);
        assert_eq!(w2.write("a/y.dng", b"new").unwrap(), d.join("y.dng.jpg"));
        assert_eq!(std::fs::read(d.join("y.jpg")).unwrap(), b"d/y.dng");
        // --overwrite replaces files from before the run, but still one name per input within the run
        let mut w3 = PreviewWriter::new(&d, true);
        assert_eq!(w3.write("a/x.arw", b"one").unwrap(), d.join("x.jpg"));
        assert_eq!(w3.write("b/x.arw", b"two").unwrap(), d.join("x.arw.jpg"));
        assert_eq!(std::fs::read(d.join("x.jpg")).unwrap(), b"one");
        assert_eq!(std::fs::read(d.join("x.arw.jpg")).unwrap(), b"two");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn written_preview_is_reported_apart_from_the_decode_status() {
        let d = scratch("written");
        let j = jpeg(12, 8);
        let bytes = tiff_with_preview(&j);
        let input = d.join("plain.tif");
        std::fs::write(&input, &bytes).unwrap();
        let out = d.join("out");
        prepare_out_dir(&out).unwrap();
        let mut w = PreviewWriter::new(&out, false);
        let (line, failed) = json_line(&input.to_string_lossy(), true, Some(&mut w));
        assert_every_key(&line, true);
        assert!(!failed);
        assert_eq!(line["status"], "unsupported");
        assert_eq!(line["error_kind"], "NotRaw");
        let p = &line["preview"];
        assert_eq!(p["status"], "written");
        assert_eq!((p["width"].as_u64(), p["height"].as_u64()), (Some(12), Some(8)));
        let written = std::fs::read(p["path"].as_str().unwrap()).unwrap();
        assert_eq!(written, embedded_preview(&bytes).unwrap());
        assert_eq!(p["bytes"].as_u64(), Some(written.len() as u64));
        assert!(line["tree"].is_array());
        // without --preview-out: found, not written
        let (line, failed) = json_line(&input.to_string_lossy(), false, None);
        assert_every_key(&line, false);
        assert!(!failed);
        assert_eq!(line["preview"]["status"], "not-requested");
        assert_eq!(line["preview"]["width"], 12);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn write_failure_is_a_preview_error_and_fails_the_run() {
        let d = scratch("fail");
        let input = d.join("plain.tif");
        std::fs::write(&input, tiff_with_preview(&jpeg(12, 8))).unwrap();
        let out = d.join("out");
        prepare_out_dir(&out).unwrap();
        let mut w = PreviewWriter::new(&out, false);
        // the directory disappears after the up-front check
        std::fs::remove_dir_all(&out).unwrap();
        let (line, failed) = json_line(&input.to_string_lossy(), false, Some(&mut w));
        assert_every_key(&line, false);
        assert!(failed);
        assert_eq!(line["status"], "unsupported");
        assert_eq!(line["preview"]["status"], "error");
        assert!(line["preview"]["error"].as_str().unwrap().contains("plain.jpg"));
        assert!(line["preview"]["path"].is_null());
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A read-only directory. Unix only: on Windows the read-only attribute of a directory does not stop files
    /// being created in it, and denying that through ACLs needs tools outside the standard library.
    #[cfg(unix)]
    #[test]
    fn read_only_destination_is_a_usage_error() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("ro");
        let out = d.join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o555)).unwrap();
        let writable = std::fs::write(out.join("probe"), b"").is_ok(); // root ignores permissions
        if !writable {
            assert!(prepare_out_dir(&out).unwrap_err().contains("cannot create files"));
        }
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn unreadable_input_has_every_key_and_fails_the_run() {
        let d = scratch("missing");
        let missing = d.join("missing.cr2").to_string_lossy().into_owned();
        let (line, failed) = json_line(&missing, true, None);
        assert_every_key(&line, true);
        assert!(failed);
        assert_eq!((line["status"].as_str(), line["error_kind"].as_str()), (Some("error"), Some("Io")));
        assert!(line["tree"].is_null());
        assert_eq!(line["preview"]["status"], "not-requested");
        let mut w = PreviewWriter::new(&d, false);
        let (line, _) = json_line(&missing, true, Some(&mut w));
        assert_every_key(&line, true);
        assert_eq!(line["preview"]["status"], "error");
        assert_eq!(run(&s(&["--json", "--tree", &missing])), 1);
        assert!(human(&missing, true, None));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn orientation_of_a_rotated_dng() {
        let d = scratch("orient");
        let input = d.join("rot.dng");
        std::fs::write(&input, rotated_dng()).unwrap();
        let (line, failed) = json_line(&input.to_string_lossy(), true, None);
        assert_every_key(&line, true);
        assert!(!failed);
        assert_eq!(line["status"], "ok", "{line}");
        assert_eq!(line["orientation"], 6);
        assert_eq!(line["orientation_name"], "Rotate90");
        assert_eq!(line["model"], "Cam 1");
        assert_eq!(line["color"]["has_color_matrix"], true);
        assert_eq!(line["preview"]["status"], "none");
        assert_eq!(run(&s(&["--json", &input.to_string_lossy()])), 0);
        std::fs::remove_dir_all(&d).unwrap();
    }
}

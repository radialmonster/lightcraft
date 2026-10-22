//! Mutation fuzz of the CIFF container parse over the CC0 CRW samples named in `LIGHTCRAFT_CRW_FILES` (`;`-separated).
use lightcraft_raw::crw;

fn files() -> Vec<std::path::PathBuf> {
    let Ok(v) = std::env::var("LIGHTCRAFT_CRW_FILES") else { return Vec::new() };
    v.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()).map(Into::into).collect()
}

#[test]
fn mutated_crw_never_panics() {
    let list = files();
    if std::env::var("LIGHTCRAFT_CRW_FILES").is_err() {
        eprintln!("LIGHTCRAFT_CRW_FILES unset: skipped");
        return;
    }
    assert!(!list.is_empty(), "LIGHTCRAFT_CRW_FILES is set but lists no paths");
    for p in &list {
        let bytes = std::fs::read(p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()));
        assert!(crw::parse(&bytes).is_ok(), "{} should parse", p.display());
        let mut s = 0x9e3779b9u32;
        for _ in 0..300 {
            let mut m = bytes[..bytes.len().min(4096)].to_vec();
            for _ in 0..8 {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                let i = (s >> 8) as usize % m.len();
                m[i] ^= (s >> 3) as u8;
            }
            let _ = crw::parse(&m);
            let _ = crw::parse(&m[..(s as usize % m.len())]);
        }
    }
}

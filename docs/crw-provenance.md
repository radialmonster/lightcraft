# CRW clean-room provenance (CC0 inputs only)

Inputs: the 11 CC0 pixls CRW files (10D 2027/3164/3165, 300D 1983/3162, D30 1307, D60 2010, Rebel 1308, G1 2073, G5 1746, G6 4226), Canon's public CIFF spec, ITU T.81. No other decoder consulted. HANDOFF-2442.md read for paths/commands/CodeRabbit items only (its facts section was incidentally displayed once; not used).

1. Container: "II", heap offset, "HEAPCCDR", dir offset in last 4 heap bytes, u16 count + 10-byte entries, inline (0x4000) and sub-dir (0x2800/0x3000) tags. Informed by: all 11 files; all parse.
2. Tag 0x1031 record gives W,H (3152x2068 10D/300D/D60/Rebel; 2224x1456 D30; 2144x1560 G1; 2672x1968 G5; 3160x2344 G6). Informed by: all 11.
3. Tag 0x2005 blob: 12-bit bodies = 2-bit plane (W*H/4) + 514 zero bytes + stream; G-series = 514 zeros + stream ending ffd9. No embedded DHT. Informed by: all 11 (zero-run analysis).
4. Stream is ff00-unstuffed; standard T.81 K.3/K.5 tables fail (bad code). Informed by: all 11.
5. 64-sample block structure (3152/64 = 49.25 units per row), repeated zero-block units, two table families (A: 10D 2027, 300D 1983, D60 2010, Rebel 1308; B: 10D 3164/3165, 300D 3162; D30/G1/G5/G6 each distinct). Informed by: those files.
6. Table inference from A/B paired starts was under-determined (thousands of consistent splits); no table derived. Entropy payload NOT decoded.
7. CodeRabbit items: visit budget (VISIT_BUDGET) + self-referencing dir test; fuzz test fails on missing/unreadable paths and on empty list; empty/truncated inputs do not panic.

## Session 2 (Lane A/B, no tables obtained)

8. Tag 0x1835 (decoder table id) is constant within a camera family (values 0/1/2), so only three table sets exist; files can be pooled per id. Informed by: all 11 CC0 files.
9. Bit-level analysis of the first blocks (run-length of 1s/0s, repeated zero-block tokens, ZRL-like 16-position tokens) supports a JPEG-like (run,size) layout, but the DC/EOB token identity and the G5/G6 framing stayed unresolved. D30 and G1 streams share an identical 17-bit prefix, then diverge. Informed by: CC0 files only.
10. Lane B: searches for an original public-domain source of the tables (patent text or standard) found nothing clearly original and clean; nothing accepted, nothing used.
11. No ncsa file was used as a derivation input. No decoder code, docs or output from any other project was opened.

Result: tables NOT derived; raw payload NOT decoded; container parse only. Scratch scripts (ciff.py, p1..p8.py) are in the session scratchpad crw/ folder.

## Session 3 (Lane A scored-decode attempt, no tables obtained)
12. Built Rust DFS table-inference tools (scratch only) over CC0 streams (10D 2027/3165 etc.) using only the CC0 files and the JPEG-like (run,size) hypothesis. Rounds logged in scratchpad crw/rounds.log. Parse-level constraints (Kraft, canonical order, block length 64, flat-border units) leave millions of consistent table sets; no candidate reached a scored decode. No decoder written, no ncsa files used, no external decoder consulted.
13. Observation: D30 raw blob does not start with a zero 2-bit plane, so the 10D/300D framing does not apply to D30.
Result: still no tables; CRW decode not implemented.
14. Hierarchical candidate scoring (coordinator request) judged infeasible with parse-only candidates: candidate table space not enumerable, 1-block windows carry no image statistics. Observations (textured-stream run statistics, 43-bit row-end block "1x14 000 1x15" core) logged in scratch rounds.log. Sources unchanged: CC0 pixls CRW only.

## Session 4 (exact phase/complement algebra, partial tables pinned)
15. Inputs: CC0 raw.pixls.us CRW files only: 10D 2027/3164/3165, 300D 1983/3162, D60 2010, Rebel 1308 (bit algebra); D30/G1/G5/G6 (JPEG row-density check, framing only). The embedded camera JPEG (tag 0x2007) of these CC0 files was used only for a row brightness/density correlation. No ncsa file, no external decoder, docs or code. Spec reference: ITU T.81 (the JPEG run/size/EOB/ZRL token idea).
16. Method: the masked top rows repeat with a 4-row period (3152 = 49.25 blocks of 64), so the row-start blocks recur at phases 0/16/32/48. Exact string algebra on these blocks plus a DC(+v)/DC(-v) complement test fixes the code boundaries.
17. Result (partial, exact): table id 0: DC size0=0, size10=10; EOB=1111110111111, ZRL=1111111011, (15,10)=1111111110100011, (0,1)=111101, plus one row-start (r,10)=1111111010. Table id 2: DC size0=0, size10=10; EOB=111111001111, ZRL=111111000, (15,10)=1111111110011001, row-start (r,10)=1111100. These codes alone decode the first 344 blocks of every tid0/tid2 CC0 file exactly. Table id 1, most AC symbols and the textured-region payload are NOT derived. Scripts: session scratchpad crw/r4/, log crw/rounds.log R8-R10.

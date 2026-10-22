# CRW clean-room provenance (CC0 inputs only)

Inputs: the 11 CC0 pixls CRW files (10D 2027/3164/3165, 300D 1983/3162, D30 1307, D60 2010, Rebel 1308, G1 2073, G5 1746, G6 4226), Canon's public CIFF spec, ITU T.81. No other decoder consulted. HANDOFF-2442.md read for paths/commands/CodeRabbit items only (its facts section was incidentally displayed once; not used).

1. Container: "II", heap offset, "HEAPCCDR", dir offset in last 4 heap bytes, u16 count + 10-byte entries, inline (0x4000) and sub-dir (0x2800/0x3000) tags. Informed by: all 11 files; all parse.
2. Tag 0x1031 record gives W,H (3152x2068 10D/300D/D60/Rebel; 2224x1456 D30; 2144x1560 G1; 2672x1968 G5; 3160x2344 G6). Informed by: all 11.
3. Tag 0x2005 blob: 12-bit bodies = 2-bit plane (W*H/4) + 514 zero bytes + stream; G-series = 514 zeros + stream ending ffd9. No embedded DHT. Informed by: all 11 (zero-run analysis).
4. Stream is ff00-unstuffed; standard T.81 K.3/K.5 tables fail (bad code). Informed by: all 11.
5. 64-sample block structure (3152/64 = 49.25 units per row), repeated zero-block units, two table families (A: 10D 2027, 300D 1983, D60 2010, Rebel 1308; B: 10D 3164/3165, 300D 3162; D30/G1/G5/G6 each distinct). Informed by: those files.
6. Table inference from A/B paired starts was under-determined (thousands of consistent splits); no table derived. Entropy payload NOT decoded.
7. CodeRabbit items: visit budget (VISIT_BUDGET) + self-referencing dir test; fuzz test fails on missing/unreadable paths and on empty list; empty/truncated inputs do not panic.

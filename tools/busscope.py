"""Analyse a CS/SCK logic-analyser capture of the W6300 SPI bus.

This is the tool that located the Art-Net ingest ceiling (2026-09-16): it showed
the payload DMA is continuous and the cost is executor latency after the last
byte - see docs/ARCHITECTURE.md section 11. Re-run it after any change to the
transport, the render path, or the executor load on core 0.

Input: a Saleae-style edge-list CSV with header  Time [s],CS,SCK  - one row per
transition. Probe CS on GP16 (module pin 21) and SCK on GP17 (module pin 22)
during  python tools/dmxsend.py --count 32 --rate 44 --pattern ramp ...

    python tools/busscope.py digital.csv
    python tools/busscope.py digital.csv --period-ms 22.7   # frames per sender period

Section E splits bursts where the driver *pauses*. Once the receiver is fast
enough to never pause, that merges bursts and the count reads high - so give
--period-ms (the sender's frame period, 1000/rate) and section E also counts
reads in fixed windows of that length, which does not depend on pauses. The
reads/s headline is robust either way.

Sections:
  B. Byte offset of every >1 us gap inside the 576-byte reads. Offsets 1, 3, 4
     are the header Operation boundaries; gaps only there mean the payload DMA
     is continuous. Also splits each read into head / header / payload / tail.
  C. Per Ethernet frame: register phase vs data phase, transactions per frame,
     and how often the RSR read-until-stable loop spins.
  D. The gaps between sub-bursts of the small register transactions - each one
     is a direct measurement of a DMA-IRQ -> wake -> resume round trip.
  E. Bursts. A controller emits all its universes back to back at each frame
     tick, so data reads cluster. Frames surviving per burst is THE number: flat
     at ~9 regardless of frame speed means the chip buffer (4 KB = 7 frames) is
     the limit; a count that rises as per-frame time falls means the drain is.
     Also reports the idle gap between bursts and the register phase with any
     inter-burst idle excluded - section C's p90 includes that idle and is
     inflated by it; use E's figure.
"""
import sys, statistics, collections

path = sys.argv[1]
period_ms = None
if "--period-ms" in sys.argv:
    period_ms = float(sys.argv[sys.argv.index("--period-ms") + 1])
T, CS, SK = [], [], []
with open(path, "r", encoding="utf-8", errors="replace") as f:
    next(f)
    for line in f:
        p = line.rstrip("\n").split(",")
        if len(p) < 3:
            continue
        try:
            T.append(float(p[0])); CS.append(int(p[1])); SK.append(int(p[2]))
        except ValueError:
            pass
n = len(T)

# ---- transactions: (start, end, rise_times) ----------------------------------
tx = []
cur = None; rises = []
prev_cs, prev_sk = CS[0], SK[0]
if prev_cs == 0:
    cur = T[0]
for i in range(1, n):
    cs, sk, t = CS[i], SK[i], T[i]
    if cs != prev_cs:
        if cs == 0:
            cur = t; rises = []
        elif cur is not None:
            tx.append((cur, t, rises)); cur = None
    if cur is not None and sk == 1 and prev_sk == 0:
        rises.append(t)
    prev_cs, prev_sk = cs, sk

def nbytes(w): return len(w[2]) // 8
def pct(sorted_list, p): return sorted_list[min(len(sorted_list) - 1, int(p * len(sorted_list)))]

big = [w for w in tx if nbytes(w) >= 300]
small = [w for w in tx if 4 <= nbytes(w) <= 8]
print(f"transactions {len(tx)}: data reads {len(big)}, register accesses {len(small)}")

# ================= B: where are the long gaps inside a data read? ============
print("\n=== B. byte offset of every gap > 1 us inside the 576-byte reads ===")
offs = collections.Counter(); total_gap_by_region = {"header (<=4)": 0.0, "data (>4)": 0.0}
per_window_extra = []; per_window_ngaps = []
for s, e, rt in big:
    nb = len(rt) // 8
    extra = 0.0; ng = 0
    for k in range(1, nb):
        g = rt[8 * k] - rt[8 * k - 1]
        if g > 1e-6:
            offs[k] += 1; ng += 1; extra += g
            total_gap_by_region["header (<=4)" if k <= 4 else "data (>4)"] += g
    per_window_extra.append(extra); per_window_ngaps.append(ng)
print("gap count by byte offset (top 12):")
for k, c in sorted(offs.items(), key=lambda kv: -kv[1])[:12]:
    print(f"  offset {k:4d}: {c:4d} gaps  ({100 * c / len(big):5.1f}% of reads)")
hdr = sum(c for k, c in offs.items() if k <= 4); dat = sum(c for k, c in offs.items() if k > 4)
print(f"gaps at header boundaries (offset<=4): {hdr}   gaps inside the data (offset>4): {dat}")
for k, v in total_gap_by_region.items():
    print(f"  total stall time {k:12s}: {v * 1e3:8.2f} ms over {len(big)} reads  ({v / len(big) * 1e6:6.1f} us/read)")
durs = sorted(e - s for s, e, rt in big)
print(f"data-read CS window: min {durs[0]*1e6:.0f}  median {pct(durs,0.5)*1e6:.0f}  p90 {pct(durs,0.9)*1e6:.0f}  max {durs[-1]*1e6:.0f} us")
wire = 576 * 8 / 20e6
print(f"pure wire time for 576 B at 20 MHz: {wire*1e6:.0f} us; median window minus wire = {(pct(durs,0.5)-wire)*1e6:.0f} us")
# does (window - wire) equal the summed long gaps?
resid = [ (e - s) - wire - x for (s, e, rt), x in zip(big, per_window_extra)]
print(f"window - wire - sum(long gaps): median {statistics.median(resid)*1e6:.1f} us  (near 0 => the long gaps explain all of it)")
head_t = [rt[0] - s for s, e, rt in big]
hdr_t  = [rt[8 * 4] - rt[0] for s, e, rt in big]
pay_t  = [rt[-1] - rt[8 * 4] for s, e, rt in big]
tail_t = [e - rt[-1] for s, e, rt in big]
print("anatomy of a 576-byte read (median / p90, us):")
for name, xs in (("CS low -> first SCK", head_t), ("header 4 B + 3 waits", hdr_t),
                 ("payload first->last SCK", pay_t), ("last SCK -> CS high (tail)", tail_t)):
    xs = sorted(xs)
    print(f"  {name:28s} {pct(xs,0.5)*1e6:8.1f} {pct(xs,0.9)*1e6:8.1f}")
print(f"  payload wire time at 20 MHz: {572*8/20e6*1e6:.1f} us - a tail far above the header waits is executor latency, not the bus")

# ================= C: per-frame accounting ==================================
print("\n=== C. per Ethernet frame: register phase vs data phase ===")
# a frame = the run of small transactions leading up to a data read, plus the
# small ones after it up to the next frame's first small one. Simplest robust
# split: assign every small transaction to the NEXT data read.
frames = []
pending = []
for w in tx:
    if nbytes(w) >= 300:
        frames.append((pending, w)); pending = []
    else:
        pending.append(w)
frames = [f for f in frames if f[0]]
reg_n = [len(p) for p, d in frames]
reg_time = [d[0] - p[0][0] for p, d in frames]          # first small CS-low to data CS-low
data_time = [d[1] - d[0] for p, d in frames]
frame_time = [d[1] - p[0][0] for p, d in frames]
def summ(name, xs, scale=1e3, unit="ms"):
    xs = sorted(xs)
    print(f"  {name:28s} median {statistics.median(xs)*scale:7.3f}  p10 {pct(xs,0.1)*scale:7.3f}  p90 {pct(xs,0.9)*scale:7.3f} {unit}")
print(f"frames: {len(frames)}")
print(f"  register transactions/frame  median {statistics.median(reg_n)}  min {min(reg_n)}  max {max(reg_n)}")
summ("register phase (p90 INCLUDES inter-burst idle - see E)", reg_time)
summ("data phase (CS window)", data_time)
summ("total (p90 INCLUDES inter-burst idle - see E)", frame_time)
# Which register accesses? sizes in order for a typical frame
mid = frames[len(frames)//2]
print("  typical frame register sizes (B):", [nbytes(w) for w in mid[0]])
# RSR loop spins: count 6-B reads before the first 5-B (interrupt clear) write
spins = []
for p, d in frames:
    k = 0
    for w in p:
        if nbytes(w) == 6: k += 1
        else: break
    spins.append(k)
print(f"  6-B reads before the first 5-B write (RSR pairs, x2): median {statistics.median(spins)}  max {max(spins)}")

# ================= D: one round-trip, measured directly =====================
print("\n=== D. gaps between sub-bursts inside 5-6 B register transactions ===")
# sub-bursts: bytes 0 | 1,2 | 3 | 4.. ; boundaries after byte 0, 2, 3
rt_gaps = collections.defaultdict(list)
for s, e, rt in small:
    nb = len(rt) // 8
    for k, label in ((1, "after instruction"), (3, "after address"), (4, "after dummy")):
        if k < nb:
            rt_gaps[label].append(rt[8 * k] - rt[8 * k - 1])
allg = []
for label, gs in rt_gaps.items():
    gs = sorted(gs); allg += gs
    print(f"  {label:18s} n={len(gs):5d}  p10 {pct(gs,0.1)*1e6:6.1f}  median {pct(gs,0.5)*1e6:6.1f}  p90 {pct(gs,0.9)*1e6:6.1f}  p99 {pct(gs,0.99)*1e6:6.1f}  max {gs[-1]*1e6:6.1f} us")
allg.sort()
print(f"  all boundaries       n={len(allg):5d}  median {pct(allg,0.5)*1e6:6.1f}  p90 {pct(allg,0.9)*1e6:6.1f}  p99 {pct(allg,0.99)*1e6:6.1f} us")
cs_to_first = sorted(rt[0] - s for s, e, rt in small if rt)
last_to_cs = sorted(e - rt[-1] for s, e, rt in small if rt)
print(f"  CS low -> first SCK   median {pct(cs_to_first,0.5)*1e6:6.1f}  p90 {pct(cs_to_first,0.9)*1e6:6.1f} us")
print(f"  last SCK -> CS high   median {pct(last_to_cs,0.5)*1e6:6.1f}  p90 {pct(last_to_cs,0.9)*1e6:6.1f} us")
between = sorted(tx[i+1][0] - tx[i][1] for i in range(len(tx)-1) if tx[i+1][0] - tx[i][1] < 300e-6)
print(f"  CS high -> next CS low median {pct(between,0.5)*1e6:6.1f}  p90 {pct(between,0.9)*1e6:6.1f} us  (task work between transactions)")

# ================= E: bursts ==================================================
print("\n=== E. bursts: how many frames survive each one, and what the driver does between ===")
reads = big
bursts = [[reads[0]]]
for a_, b_ in zip(reads, reads[1:]):
    # a new burst starts when the gap from one read's end to the next read's start
    # exceeds 5 ms - inside a burst reads follow each other within ~1 ms
    if b_[0] - a_[1] > 5e-3:
        bursts.append([b_])
    else:
        bursts[-1].append(b_)
inner = [bb for bb in bursts if len(bb) >= 2]
per = [len(bb) for bb in bursts]
span = sorted(bb[-1][1] - bb[0][0] for bb in inner)
perframe = sorted((bb[-1][1] - bb[0][0]) / (len(bb) - 1) for bb in inner)
idle = sorted(bursts[i + 1][0][0] - bursts[i][-1][1] for i in range(len(bursts) - 1))
if len(bursts) < 3:
    print("  the driver never paused for >5 ms in this capture: it is drain-limited or at the edge,")
    print("  so pause-split bursts are meaningless here. Use the fixed-window count below.")
    print(f"  per-frame time, whole capture: median {pct(perframe,0.5)*1e3:.3f} ms" if perframe else "")
else:
    print(f"  bursts {len(bursts)}   frames surviving per burst: median {statistics.median(per):.0f}  min {min(per)}  max {max(per)}")
    print(f"  burst drain span (first read start -> last read end): median {pct(span,0.5)*1e3:.2f} ms  p90 {pct(span,0.9)*1e3:.2f} ms")
    print(f"  per-frame time inside a burst: median {pct(perframe,0.5)*1e3:.3f} ms  p90 {pct(perframe,0.9)*1e3:.3f} ms")
    if idle:
        print(f"  idle between bursts: median {pct(idle,0.5)*1e3:.1f} ms  p10 {pct(idle,0.1)*1e3:.1f}  p90 {pct(idle,0.9)*1e3:.1f} ms   (44 Hz => 22.7 ms period)")
    print(f"  => implied rate at 44 bursts/s: {statistics.median(per) * 44:.0f} frames/s")
# register phase per frame with any single gap > 2 ms (inter-burst idle) excluded
regs = []; pend = []
for w in tx:
    if nbytes(w) >= 300:
        if pend:
            busy = sum(e - s_ for s_, e, _ in pend)
            gaps = [pend[i + 1][0] - pend[i][1] for i in range(len(pend) - 1)] + [w[0] - pend[-1][1]]
            regs.append(busy + sum(g for g in gaps if g < 2e-3))
        pend = []
    else:
        pend.append(w)
regs.sort()
print(f"  register phase per frame, idle excluded: median {pct(regs,0.5)*1e3:.3f} ms  p90 {pct(regs,0.9)*1e3:.3f} ms  max {regs[-1]*1e3:.3f} ms")
print("  reading: a burst count flat at ~9 whatever the frame speed = the 4 KB chip buffer is the ceiling;")
print("           a count that rises as per-frame time falls = the drain rate is.")
span_s = T[-1] - T[0]
print(f"  headline: {len(reads)} data reads in {span_s:.2f} s = {len(reads)/span_s:.0f} frames/s (robust to how bursts are split)")
bp = sorted(bursts[i + 1][0][0] - bursts[i][0][0] for i in range(len(bursts) - 1))
if bp:
    med_bp = pct(bp, 0.5) * 1e3
    expect = period_ms if period_ms else 22.7
    if abs(med_bp - expect) > 0.2 * expect:
        print(f"  !! pause-split burst period is {med_bp:.1f} ms against an expected {expect:.1f}: either the")
        print("     sender is irregular (check its own spacing report) or the driver never pauses, so")
        print("     bursts above are MERGED and their counts are too high. Use --period-ms.")
if period_ms:
    w = period_ms / 1e3
    w0 = reads[0][0]
    counts = collections.Counter(int((r[0] - w0) // w) for r in reads)
    full = [counts.get(k, 0) for k in range(int(span_s // w))]
    if full:
        fs = sorted(full)
        print(f"  frames per {period_ms:.1f} ms window (sender period, independent of pauses): "
              f"median {statistics.median(fs):.0f}  p10 {pct(fs,0.1)}  p90 {pct(fs,0.9)}  min {fs[0]}  max {fs[-1]}")
        print("           with a regular sender this is survivors per burst; 32 offered means 32 is the target")

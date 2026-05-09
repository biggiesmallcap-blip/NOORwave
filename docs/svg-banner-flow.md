# SVG Banner — EQ Animation Flow

Reference document for the maths and constants behind the animated NOOR EQ-bar
visualisation that appears in [`frontend/static/noor-logo-centered.svg`](../frontend/static/noor-logo-centered.svg)
and the banner variants under [`frontend/static/social/`](../frontend/static/social/).

If you regenerate or tweak these SVGs, follow this pipeline. Skipping steps or
reordering them produces the visual artefacts catalogued in §7.

---

## 1. The visualisation

20 vertical "LED bars" sit at fixed x-positions inside the wordmark. Each bar
animates its `height` attribute over a 3.6 s loop using SMIL `<animate>`. The
bars are clipped by a downward-pointing isoceles triangle so the strip tapers
to a point under the centre of the wordmark.

| Constant | Value | Source |
|----------|-------|--------|
| Bars | 20 | matches asset-pack design |
| Cycle duration | 3.6 s | tuned for "relaxed pulse" tempo |
| Per-bar keyframes | 128 (+ 1 seam closure = 129) | dense enough that linear segments are < 30 ms each, below visual stepping threshold |
| Bar width | 34.5 px | source |
| Bar baseline y | 600 | bars extend downward from this y |
| LED pattern | `<pattern id="led">` 40 × 10 with 7-tall white slabs | striped fill imitates an LED bar-graph |
| Clip-triangle vertices | `(262,590) (1422,590) (842,1000)` | wide top, apex at centre-bottom |

### Bar x-positions

```
402.0, 446.5, 491.0, 535.5, 580.0,
624.5, 669.0, 713.5, 758.0, 802.5,
847.0, 891.5, 936.0, 980.5, 1025.0,
1069.5, 1114.0, 1158.5, 1203.0, 1247.5
```

Spacing is 44.5 px, centred under the wordmark glyphs.

---

## 2. The clip envelope

A bar at horizontal position $x$ may not be taller than the distance from
$y=600$ to where the clip triangle's diagonal edge crosses $x$.

The triangle's two diagonal edges have slope $|m| = \frac{1000-590}{842-262}
= \frac{410}{580} \approx 0.7069$.

$$
\text{clip\_y}(x) =
\begin{cases}
590 + \frac{410}{580}\,(x - 262) & 262 \le x \le 842 \quad \text{(left edge)} \\
1000 - \frac{410}{580}\,(x - 842) & 842 \le x \le 1422 \quad \text{(right edge)}
\end{cases}
$$

$$
\text{max\_height}(x) = \text{clip\_y}(x) - 600
$$

To prevent any bar visibly hitting the diagonal (which reads as the bar
"sticking" mid-animation), we apply a safety margin of 0.85:

$$
\text{cap}(x) = 0.85 \cdot \text{max\_height}(x)
$$

| Bar | x | `clip_y(x)` | `max_height` | `cap` (×0.85) |
|----:|----:|----:|----:|----:|
| 0 | 402.0 | 689 | 89 | 76 |
| 1 | 446.5 | 720 | 120 | 102 |
| 2 | 491.0 | 752 | 152 | 129 |
| ⋯ | | | | |
| 9 | 802.5 | 982 | 382 | 325 |
| 10 | 847.0 | 996 | 396 | 337 |
| 11 | 891.5 | 965 | 365 | 310 |
| ⋯ | | | | |
| 18 | 1203.0 | 745 | 145 | 123 |
| 19 | 1247.5 | 715 | 115 | 96 |

The middle bars get vastly more vertical room (cap ≈ 337) than the outer bars
(cap ≈ 76). Per-bar amplitude scaling (§4) uses these caps directly.

---

## 3. Source data

The 20 hand-crafted keyframe sequences come from
`E:\noor_asset_pack\svg\noor-logo-centered.svg`. The asset-pack source ships
each bar as 25 frames where `frame[24] == frame[0]` — the trailing duplicate
that makes the sequence a closed loop. We treat the unique 24 frames as
periodic data; the duplicate is handled by the seam-closure rule (§6).

Naïve approaches that fail:

- **Use the source values directly.** With `calcMode="discrete"`, the duplicated
  closing frame causes that value to be held for two consecutive 100 ms slots
  → visible stutter at the loop point.
- **Drop the duplicate, switch to `calcMode="linear"`.** This works for value
  continuity at the seam but not for *velocity* continuity. Bars that were
  rising into the seam frame and falling out of it create a hard direction
  reversal once per loop → visible jolt.
- **Use the source data at full amplitude.** Bars 0, 1, 2, 3, 16, 17, 18, 19
  exceed their clip envelope (§2) and visibly stick on the diagonal for some
  frames mid-animation.

The pipeline below addresses all three.

---

## 4. The pipeline

Apply these steps in order; do not iterate them. Each pass is destructive
(integer rounding compounds quickly), so do them all in one float-precision
chain and round only at the end.

### 4.1 Gaussian time-domain smoothing

For each bar's 24 source values $v_0 \ldots v_{23}$, treat as periodic and
convolve with a Gaussian kernel:

$$
v'_i = \sum_{j} K(j;\,\sigma)\,v_{(i+j) \bmod 24}
\qquad K(j;\,\sigma) = \frac{1}{\sigma\sqrt{2\pi}} e^{-j^2/(2\sigma^2)}
$$

with **σ = 1.5** keyframes. This kills single-frame spikes (where a bar's
source data has $|\Delta| > 5\sigma$ across one keyframe boundary) while
preserving the low-frequency wave structure that gives the strip its
recognisable EQ flow.

In Python: `scipy.ndimage.gaussian_filter1d(arr, sigma=1.5, mode='wrap')`.

### 4.2 Per-bar amplitude scaling

For each bar $i$:

$$
s_i = \min\!\left(0.65,\ \ \frac{\text{cap}(x_i)}{\max_j v'_j}\right)
$$

Then $v''_i = s_i \cdot v'_i$.

The 0.65 ceiling prevents middle bars (which have cap ≈ 337) from being scaled
*up* — we want their amplitude to remain expressive but bounded. Outer bars,
whose source values exceed their cap at any reasonable scale, get a tighter
$s_i$ (typically ~0.49 for bars 0 and 19).

### 4.3 C² periodic cubic spline

Fit a cubic spline through the 24 smoothed, scaled values with **periodic
boundary conditions** (matching first *and* second derivatives at the seam):

$$
S \in C^2,\ \ S(0) = S(24),\ \ S'(0) = S'(24),\ \ S''(0) = S''(24)
$$

In Python: `scipy.interpolate.CubicSpline(t_keys, y_closed, bc_type='periodic')`.

This is the critical step. C² continuity means there is **no slope change and
no curvature change** anywhere on the curve — including the seam. Catmull-Rom
or Hermite splines only guarantee C¹; the second derivative jump at keyframes
is small but visible as a faint wobble in the bar's motion under sustained
viewing.

### 4.4 Dense uniform resampling

Sample the spline at **128 equally-spaced points** over the cycle. Round to
integer at this stage; clamp at a floor of **4** (any value below 4 produces
visually-identical "near zero" bars while keeping a single LED slab visible
to anchor the bar's position).

Per-segment SMIL duration becomes $3600 / 128 \approx 28.1$ ms — short enough
that the linear interpolation SMIL applies between keyframes is below the
human flicker-fusion threshold for fine spatial detail.

### 4.5 Seam closure

Append `values[0]` as the 129th frame:

$$
\text{values}[128] := \text{values}[0]
$$

This is required by SMIL's `<animate>` behaviour. With `calcMode="linear"`
(default), SMIL spreads the $N$ values evenly across `dur`. At $t = \text{dur}$
the position is `values[N-1]`; on the loop restart at $t = 0$ the position is
`values[0]`. For a continuous loop these must be equal.

### 4.6 Global phase rotation

Pick the rotation $K \in [0, 128)$ that places the loop seam at the
**highest mean-height frame** across all 20 bars:

$$
K = \arg\max_k \ \frac{1}{20}\sum_{i=0}^{19} v''_i\!\!\left((-k) \bmod 128\right)
$$

Then apply $K$ uniformly to every bar. Because the rotation is *global*
(same $K$ for all bars), the relative phase between any two bars is preserved
exactly — the wave structure that flows left-to-right across the strip is
unchanged. Only the choice of which moment in that flow happens to coincide
with the loop boundary changes.

This is what eliminates the "synchronised dip at the seam" failure mode: by
construction the seam now falls at the cycle's *peak* mean energy, not its
trough.

---

## 5. SMIL output format

Each bar emits one `<animate>` element:

```xml
<rect x="624.5" y="600" width="34.5" height="98" fill="url(#led)">
  <animate attributeName="height"
           values="98;100;102;...;96;98"
           dur="3.6s"
           repeatCount="indefinite"/>
</rect>
```

Notes:

- **No `calcMode` attribute** → defaults to `linear` interpolation between
  keyframes. Do not specify `discrete` (causes loop stutter, see §3).
- **`values` is 129 ints separated by `;`**, last == first.
- **The static `height=` attribute** on the `<rect>` should equal `values[0]`
  so the bar renders correctly before SMIL takes over (e.g. on first paint or
  in environments without SMIL support).

---

## 6. Banner variants

Animated SVG banners under [`frontend/static/social/`](../frontend/static/social/)
all wrap the same animated content via a uniform-scale group:

```xml
<svg viewBox="0 0 W H" preserveAspectRatio="xMidYMid meet">
  <g transform="translate(tx ty) scale(s)">
    <!-- inner content of noor-logo-centered.svg -->
  </g>
</svg>
```

For target viewBox $W \times H$ and source content $1600 \times 860$ with a
desired logo coverage $c = 0.90$:

$$
s = \min\!\left(\frac{W \cdot c}{1600},\ \frac{H \cdot c}{860}\right)
\qquad
t_x = \frac{W - 1600\,s}{2}
\qquad
t_y = \frac{H - 860\,s}{2}
$$

Resulting scale per banner:

| Banner | viewBox | scale `s` |
|--------|---------|----------:|
| `github-card.svg` | 1280 × 640 | 0.670 |
| `og-card.svg` | 1200 × 630 | 0.659 |
| `x-banner.svg` | 1500 × 500 | 0.523 |
| `square.svg` | 1080 × 1080 | 0.608 |

The inner content is byte-identical across all five files — verify with the
EQ-animation hash:

```python
import hashlib, re
text = open(path).read()
h = hashlib.sha1(''.join(re.findall(r'<animate[^>]*values="([^"]+)"', text)).encode()).hexdigest()[:12]
# Should equal 3fecbfbcd59c (current finalised flow, 2026-05-09)
```

If this hash diverges, run the pipeline (§4) to regenerate every variant
together. Never edit the keyframes of one banner in isolation.

---

## 7. Failure modes & their fingerprints

When something looks wrong, identify which constraint was broken:

| Symptom | Likely cause |
|---------|--------------|
| Hitch / freeze for ~100 ms once per loop | Trailing duplicate kept under `calcMode="discrete"` (§3) |
| Direction reversal at seam | `v[N-1] == v[0]` but velocity not matched (skipped §4.3 — used C¹ spline) |
| Bars under N or R "stick" partway through their growth | Skipped per-bar amplitude scaling (§4.2) — outer bars exceed their clip envelope |
| Synchronised collapse at the loop point, especially under O₁ and O₂ | Skipped global rotation (§4.6) — original asset has bars 5, 6, 13 bottoming at frame 0 |
| One bar suddenly accelerates at one moment in the cycle | Skipped Gaussian smoothing (§4.1) — source data has a single-frame spike, spline reproduces it as fast smooth motion |
| Visible stairstepping / chunky motion | Too few keyframes — increase target sample count past 128 |
| Wave coherence lost — bars dance independently | Bars rotated *individually* instead of *globally* — only the global rotation in §4.6 preserves relative phase |

---

## 8. Reproduction script

The full pipeline lives in this commit's working notes. To regenerate from the
asset-pack source, the relevant invariants are:

```python
HEIGHT_SCALE_CAP = 0.65          # §4.2
SAFETY           = 0.85          # §2 cap multiplier
SMOOTH_SIGMA     = 1.5           # §4.1
TARGET_FRAMES    = 128           # §4.4
SEAM_FLOOR       = 4             # §4.4
NEW_DUR          = '3.6s'        # §1
SOURCE_SVG       = 'noor_asset_pack/svg/noor-logo-centered.svg'
```

Pseudocode:

```
for each bar in 20 bars:
    v  = read_source_values(bar)            # 25 frames, drop duplicate -> 24
    v  = gaussian_filter_periodic(v, σ)     # §4.1
    s  = min(0.65, cap[bar] / max(v))       # §4.2
    v  = v * s
    cs = CubicSpline(t_keys, v + [v[0]], bc_type='periodic')   # §4.3
    v  = sample_spline(cs, 128 points)      # §4.4
    v  = round_int_floor4(v)
    v  = v + [v[0]]                         # §4.5
    bars_out[bar] = v

K = argmax over k of  mean(bars_out[i][(-k) mod 128] for i in 0..19)
for each bar: bars_out[bar] = rotate(bars_out[bar], K)   # §4.6

write_svg(bars_out)
```

Validation after regeneration:

```
for each bar:
    assert max(bar) <= cap[i]                              # §2 satisfied
    assert bar[-1] == bar[0]                               # §4.5 seam
    assert abs(end_velocity - start_velocity) <= 1         # §4.3 working
    assert max(|deltas|) / median(|deltas|) <= 4           # §4.1 working
```

If any assertion fails, the corresponding pipeline step regressed.

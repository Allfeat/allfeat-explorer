<script setup lang="ts">
// Dashboard hero — a continuous waveform of recent blocks ("the network,
// in motion"). Each bar = one block, height encodes activity, the row
// scrolls left smoothly during each 6 s block cycle so the head stays
// pinned right and a new bar slides in when the next block lands.
//
// Bar height = weighted, min-max-normalised score over the visible 72-
// block window (extrinsics + events + ref-time saturation). Min-max
// gives visible relief even on a static chain where absolute counts
// barely move; the floor (0.15) keeps every bar legible.
//
// Smooth scroll is driven by requestAnimationFrame so it pauses on
// background tabs and stays display-synced. SSR initialises now = 0 so
// the first paint has cyclePos = 0 (no transform, hydration-clean); the
// rAF loop kicks in on mount.
//
// Live updates piggy-back on the existing block WS push: the store's
// pushBlock action mirrors each frame into the waveform buffer, so the
// component just reads `blocks` reactively and re-renders.

import type { Block, NetworkSpec, WaveformBlock } from '@bindings'

const props = defineProps<{
  blocks: WaveformBlock[]
  head: Block | null
  network: NetworkSpec | null
  finalizedHeadNumber: string | null
}>()

// Bar count adapts to the viewport so each bar stays fat enough to
// click/tap. SSR + first client paint render the desktop default (72) so
// hydration matches; an onMounted listener downshifts on mobile right
// after mount. Keeping the SSR default means we don't need <ClientOnly>
// around the waveform (which would break the above-the-fold SSR paint).
const DESKTOP_BARS = 72
const TABLET_BARS = 48
const MOBILE_BARS = 32
const BARS = ref(DESKTOP_BARS)

let mqSm: MediaQueryList | null = null
let mqMd: MediaQueryList | null = null
function recomputeBars() {
  if (!mqSm || !mqMd) return
  BARS.value = mqSm.matches ? MOBILE_BARS : mqMd.matches ? TABLET_BARS : DESKTOP_BARS
}

onMounted(() => {
  mqSm = window.matchMedia('(max-width: 640px)')
  mqMd = window.matchMedia('(max-width: 900px)')
  recomputeBars()
  mqSm.addEventListener('change', recomputeBars)
  mqMd.addEventListener('change', recomputeBars)
})

onBeforeUnmount(() => {
  mqSm?.removeEventListener('change', recomputeBars)
  mqMd?.removeEventListener('change', recomputeBars)
})

const { queryString, to } = useNetworkLink()
const addrLabel = useAddrLabel()

const blockTimeSecs = computed<number>(() => {
  const raw = props.network?.block_time_secs
  if (!raw) return 6
  const parsed = Number(raw)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 6
})

// FNV-1a → [0, 1). Stable per input so the quiet-chain jitter below
// stays the same across re-renders (no flickering bars).
function hash01(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) {
    h = Math.imul(h ^ s.charCodeAt(i), 16777619)
  }
  return ((h >>> 0) % 10000) / 10000
}

// Per-bar height in [0.15, 1.0]. Each component is min-max normalised
// over the visible window so a static chain with little absolute
// variation still shows relief.
const heights = computed<number[]>(() => {
  const blocks = props.blocks
  if (blocks.length === 0) return []

  const exts = blocks.map(b => b.extrinsic_count)
  const evts = blocks.map(b => b.event_count)
  const refs = blocks.map(b => b.ref_time_pct)

  const minMax = (arr: number[]): [number, number] => {
    let lo = arr[0]!
    let hi = arr[0]!
    for (let i = 1; i < arr.length; i++) {
      const v = arr[i]!
      if (v < lo) lo = v
      if (v > hi) hi = v
    }
    return [lo, hi]
  }

  const [extLo, extHi] = minMax(exts)
  const [evtLo, evtHi] = minMax(evts)
  const [refLo, refHi] = minMax(refs)

  // Quiet-chain fallback: identical metrics across the window collapse
  // every bar to the same height. Substitute a stable per-block hash so
  // the waveform still reads as alive — cosmetic only, real counts are
  // always shown in the hover card so this can't mislead.
  const allFlat = extLo === extHi && evtLo === evtHi && refLo === refHi

  // Partially-flat metric still falls back to 0.5 (middle), not 0
  // (invisible) — keeps the contribution neutral without skewing.
  const norm = (v: number, lo: number, hi: number): number => {
    if (hi === lo) return 0.5
    return (v - lo) / (hi - lo)
  }

  return blocks.map((b) => {
    if (allFlat) return 0.35 + hash01(b.number) * 0.4
    const score
      = 0.4 * norm(b.extrinsic_count, extLo, extHi)
      + 0.4 * norm(b.event_count, evtLo, evtHi)
      + 0.2 * norm(b.ref_time_pct, refLo, refHi)
    return 0.15 + score * 0.85
  })
})

interface Bar {
  i: number
  block: WaveformBlock
  h: number
}

// Bar geometry — derived from the live BARS count so mobile/desktop
// slots re-size cleanly. SLOT_W stays anchored to the SVG's 1000-unit
// viewBox; preserveAspectRatio="none" stretches it to the container's
// actual pixel width.
const slotW = computed(() => 1000 / BARS.value)
const barW = computed(() => slotW.value * 0.7)
const barInset = computed(() => slotW.value * 0.15)
const barRx = computed(() => Math.min(2, barW.value / 3))
const AXIS_Y = 130
const MAX_BAR_H = 120
const SKEL_BASE_H = 20

// Layout: BARS slots, head pinned to the right. Store gives newest-first
// so we reverse for display. Head lands at i = BARS-1; short buffers
// leave lower indices empty (the skeleton layer covers those slots
// underneath).
const realBars = computed<Bar[]>(() => {
  const n = BARS.value
  const arr = props.blocks.slice(0, n).reverse()
  const hs = heights.value.slice(0, n).reverse()
  const pad = n - arr.length
  return arr.map((block, j) => ({ i: pad + j, block, h: hs[j] ?? 0.18 }))
})

// Skeleton → data crossfade. Full opacity with no data, ramping off as
// real bars populate. The ramp is deliberately steep (done by 8 blocks)
// so the skeleton doesn't linger behind a half-full waveform; the real
// bars' own enter animation handles the "fill-in" feel past that point.
const skeletonOpacity = computed<number>(() => {
  const n = props.blocks.length
  if (n === 0) return 1
  if (n >= 8) return 0
  return Math.max(0, 1 - n / 8)
})

const headWaveform = computed<WaveformBlock | null>(() => props.blocks[0] ?? null)

// Hover state — tracks which bar the cursor is over so the floating
// head card can morph into a "selected block" indicator and slide
// toward the targeted bar. `targetCardX` is the destination translation
// (px, leftward from default right-anchored position) the rAF lerp
// chases. `currentCardX` is the smoothed value applied to transform.
const stageRef = ref<HTMLElement | null>(null)
const cardRef = ref<HTMLElement | null>(null)
const isHovering = ref(false)
const hoverBlock = ref<WaveformBlock | null>(null)
const targetCardX = ref(0)
const currentCardX = ref(0)

// SSR-safe wall clock: 0 on the server (cyclePos = 0, no transform), then
// rAF starts on mount and advances every frame. Same loop also lerps the
// floating card toward its hover target so we don't multiply rAF handles.
const now = ref<number>(0)
let rafId: number | null = null

const tick = () => {
  now.value = Date.now()
  // Lerp card x toward target. Snap when within sub-pixel range so the
  // computed transform stops re-flowing once we've arrived.
  const diff = targetCardX.value - currentCardX.value
  if (Math.abs(diff) > 0.2) {
    currentCardX.value += diff * 0.18
  } else if (currentCardX.value !== targetCardX.value) {
    currentCardX.value = targetCardX.value
  }
  rafId = requestAnimationFrame(tick)
}

onMounted(() => {
  now.value = Date.now()
  rafId = requestAnimationFrame(tick)
})

onBeforeUnmount(() => {
  if (rafId !== null) {
    cancelAnimationFrame(rafId)
    rafId = null
  }
})

// Default card layout reference values (kept in sync with the .wh-head
// CSS): right-anchored from the stage's right edge with enough breathing
// room from the playhead so the head card reads as "anchored on the
// right side" without hugging the edge. The fallback width matches the
// CSS min-width so SSR and the first frame agree before the actual
// element is measurable.
const CARD_RIGHT_OFFSET = 96
const CARD_DEFAULT_WIDTH = 220
// Gap between the cursor and the card edge — keeps the card off the
// cursor so it doesn't feel glued, while still reading as "this card is
// about that bar". 36 px feels like a comfortable arm's length.
const CURSOR_GAP = 36
// Stage-edge breathing room. The card gets clamped to never go closer
// than this to either side, so the flip kicks in early enough to keep
// the whole card visible.
const STAGE_MARGIN = 16

function onStageMouseMove(e: MouseEvent) {
  const stage = stageRef.value
  if (!stage) return
  const rect = stage.getBoundingClientRect()
  const mouseX = e.clientX - rect.left
  if (mouseX < 0 || mouseX > rect.width) return

  const totalBars = BARS.value
  const barWidth = rect.width / totalBars
  const barIndex = Math.min(totalBars - 1, Math.max(0, Math.floor(mouseX / barWidth)))
  // realBars only holds slots with actual block data (head-aligned, so
  // they occupy the rightmost slots). Map from cursor slot to realBars
  // offset; short buffers leave the lower slots as skeleton-only, and
  // the out-of-range case preserves the current selection rather than
  // flickering back to head mid-stage.
  const pad = totalBars - realBars.value.length
  const offset = barIndex - pad
  if (offset < 0 || offset >= realBars.value.length) return
  const bar = realBars.value[offset]
  if (!bar) return
  isHovering.value = true
  hoverBlock.value = bar.block

  // Side-of-cursor placement. Default to the *right* of the cursor (the
  // card lives on the right by default, so this minimises travel for
  // the common case). Flip to the *left* when there isn't enough room
  // to fit the whole card to the right of the cursor without spilling
  // past STAGE_MARGIN. The flip happens once and the lerp interpolates
  // through it, so the card glides over the cursor instead of teleporting.
  const cardWidth = cardRef.value?.offsetWidth ?? CARD_DEFAULT_WIDTH
  const rightLeftEdge = mouseX + CURSOR_GAP
  const fitsRight = rightLeftEdge + cardWidth + STAGE_MARGIN <= rect.width

  let cardLeft = fitsRight
    ? rightLeftEdge
    : mouseX - CURSOR_GAP - cardWidth
  // Final clamp so a wide card on a narrow stage still stays in view
  // even when neither side has full room — prefer keeping the left
  // edge visible.
  cardLeft = Math.max(STAGE_MARGIN, Math.min(rect.width - cardWidth - STAGE_MARGIN, cardLeft))

  // Translate is expressed as a delta from the default right-anchored
  // position so the CSS `right: 32px` keeps owning the rest state.
  const defaultLeft = rect.width - CARD_RIGHT_OFFSET - cardWidth
  targetCardX.value = cardLeft - defaultLeft
}

function onStageMouseLeave() {
  isHovering.value = false
  hoverBlock.value = null
  targetCardX.value = 0
}

const cardTransform = computed<string>(
  () => `translate(${currentCardX.value.toFixed(1)}px, -50%)`,
)

const displayBlock = computed<WaveformBlock | null>(
  () => (isHovering.value && hoverBlock.value) ? hoverBlock.value : headWaveform.value,
)

// Progress (0..1) through the current block cycle, anchored on the head
// block's timestamp. Clamped to 1 so the row doesn't over-shift past one
// bar width if a block is late — the playhead just sits at the right
// edge until the next push lands.
const cyclePos = computed<number>(() => {
  const h = headWaveform.value
  if (!h || now.value === 0) return 0
  const elapsedMs = now.value - h.timestamp_ms
  const perMs = blockTimeSecs.value * 1000
  return Math.min(1, Math.max(0, elapsedMs / perMs))
})

const nextIn = computed<number>(() => {
  const perSecs = blockTimeSecs.value
  return Math.max(0, perSecs - cyclePos.value * perSecs)
})

const barShiftX = computed<number>(() => -cyclePos.value * slotW.value)

function tooltipFor(b: WaveformBlock | null): string | undefined {
  if (!b) return undefined
  return `Block #${b.number} — ${b.extrinsic_count} ext · ${b.event_count} evt · ${b.ref_time_pct}% ref${b.finalized ? ' · finalized' : ''}`
}

async function goToBlock(num: string) {
  await navigateTo(to(`/blocks/${num}`))
}

async function goToHead() {
  if (headWaveform.value) await goToBlock(headWaveform.value.number)
}

async function goToDisplayed() {
  const target = displayBlock.value
  if (target) await goToBlock(target.number)
}
</script>

<template>
  <div class="waveform-hero">
    <!-- Top meta strip: network identity + live indicator + countdown -->
    <div class="wh-top">
      <div class="wh-meta">
        <LiveDot />
        <span class="ui-label">{{ network?.name || 'Allfeat' }}</span>
        <template v-if="network?.spec_version">
          <span class="wh-sep">·</span>
          <span class="ui-label">{{ network.spec_name }} v{{ network.spec_version }}</span>
        </template>
        <template v-if="finalizedHeadNumber">
          <span class="wh-sep">·</span>
          <span class="ui-label">Finalized #{{ fmtInt(finalizedHeadNumber) }}</span>
        </template>
      </div>
      <div class="wh-countdown">
        <span class="ui-label">Next block</span>
        <span class="mono wh-next-num">{{ nextIn.toFixed(1) }}s</span>
      </div>
    </div>

    <!-- Main waveform stage -->
    <div ref="stageRef" class="wh-stage" @mousemove="onStageMouseMove" @mouseleave="onStageMouseLeave">
      <svg class="wh-svg" viewBox="0 0 1000 260" preserveAspectRatio="none">
        <defs>
          <linearGradient id="wh-bar-grad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="var(--wh-bar-top)" />
            <stop offset="100%" stop-color="var(--wh-bar-bot)" />
          </linearGradient>
          <!-- Mirrored gradient for the below-axis half so both rects are
               darkest at their outer extremity and fade to the same tone
               at the axis. Using the top gradient unchanged would put the
               strong colour at the axis for the bottom rect, reading as a
               fading reflection rather than a symmetric waveform. -->
          <linearGradient id="wh-bar-grad-mirror" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="var(--wh-bar-bot)" />
            <stop offset="100%" stop-color="var(--wh-bar-top)" />
          </linearGradient>
          <linearGradient id="wh-sweep" x1="0" y1="0" x2="1" y2="0">
            <stop offset="0%" stop-color="var(--red-500)" stop-opacity="0" />
            <stop offset="50%" stop-color="var(--red-500)" stop-opacity="0.25" />
            <stop offset="100%" stop-color="var(--red-500)" stop-opacity="0" />
          </linearGradient>
          <clipPath id="wh-clip">
            <rect x="0" y="0" width="1000" height="260" />
          </clipPath>
        </defs>

        <line x1="0" :y1="AXIS_Y" x2="1000" :y2="AXIS_Y" stroke="var(--wh-axis)" stroke-width="1" stroke-dasharray="2 4" />

        <g clip-path="url(#wh-clip)">
          <!-- Skeleton layer: always mounted, sits outside the cycle-scroll
               transform so it stays put while real bars shift across it.
               Each bar breathes on a staggered CSS animation — the wave
               propagates left-to-right and loops seamlessly (72 × 33 ms =
               one full breathing cycle). Fades out as the real buffer
               fills past a handful of blocks. -->
          <g class="wh-skeleton-layer" :style="{ opacity: skeletonOpacity }" aria-hidden="true">
            <g
              v-for="n in BARS"
              :key="`skel-${n - 1}`"
              class="wh-skel-bar"
              :style="{ '--bar-i': n - 1, '--bar-total': BARS }"
            >
              <rect
                :x="(n - 1) * slotW + barInset"
                :y="AXIS_Y - SKEL_BASE_H"
                :width="barW"
                :height="SKEL_BASE_H"
                :rx="barRx"
                fill="var(--wh-bar-skel)"
              />
              <rect
                :x="(n - 1) * slotW + barInset"
                :y="AXIS_Y"
                :width="barW"
                :height="SKEL_BASE_H"
                :rx="barRx"
                fill="var(--wh-bar-skel)"
              />
            </g>
          </g>

          <g :transform="`translate(${barShiftX}, 0)`">
            <template v-for="b in realBars" :key="b.block.number">
              <g
                class="wh-bar wh-bar--real"
                :style="{ '--bar-i': b.i, '--bar-total': BARS }"
                :opacity="b.i < 6 ? 0.3 + (b.i / 6) * 0.7 : 1"
                @click="goToBlock(b.block.number)"
              >
                <title>{{ tooltipFor(b.block) }}</title>
                <rect
                  :x="b.i * slotW + barInset"
                  :y="AXIS_Y - Math.max(2, b.h * MAX_BAR_H)"
                  :width="barW"
                  :height="Math.max(2, b.h * MAX_BAR_H)"
                  :rx="barRx"
                  :fill="b.i === BARS - 1
                    ? 'var(--red-500)'
                    : b.i >= BARS - 4
                      ? 'var(--wh-bar-near)'
                      : 'url(#wh-bar-grad)'"
                />
                <rect
                  :x="b.i * slotW + barInset"
                  :y="AXIS_Y"
                  :width="barW"
                  :height="Math.max(2, b.h * MAX_BAR_H)"
                  :rx="barRx"
                  :fill="b.i === BARS - 1
                    ? 'var(--red-500)'
                    : b.i >= BARS - 4
                      ? 'var(--wh-bar-near)'
                      : 'url(#wh-bar-grad-mirror)'"
                />
              </g>
            </template>
          </g>
          <!-- Cycle sweep. Hidden on SSR / first paint (now=0 → cyclePos=0 →
               sweep parked at x=-120 off-stage) and faded in once rAF kicks
               in. Without this, the sweep "pops" from off-screen to its real
               position on hydration and the red overlay reads as the bars
               suddenly darkening. -->
          <rect
            class="wh-sweep"
            :x="cyclePos * 1000 - 120"
            y="0"
            width="240"
            height="260"
            fill="url(#wh-sweep)"
            :style="{ opacity: now > 0 ? 1 : 0 }"
          />
        </g>

        <g>
          <line x1="980" y1="0" x2="980" y2="260" stroke="var(--red-500)" stroke-width="1.5" stroke-opacity="0.7" />
          <circle cx="980" cy="130" r="4" fill="var(--red-500)">
            <animate attributeName="r" values="4;7;4" dur="1.2s" repeatCount="indefinite" />
            <animate attributeName="fill-opacity" values="1;0.4;1" dur="1.2s" repeatCount="indefinite" />
          </circle>
        </g>
      </svg>

      <!-- Floating head-block card. Doubles as a hover indicator: when
           the cursor is over a bar, the card slides toward it (rAF lerp
           on `currentCardX`) and morphs to show the targeted block.
           Returns to the right anchor + head info on mouse leave. -->
      <div
        v-if="displayBlock"
        ref="cardRef"
        class="wh-head"
        :class="{ 'wh-head--hover': isHovering }"
        :style="{ transform: cardTransform }"
        @click="goToDisplayed"
      >
        <div class="ui-label wh-head-label">
          <span v-if="isHovering">Block · selected</span>
          <span v-else>Head block · live</span>
        </div>
        <div class="wh-head-num">
          #<span :key="displayBlock.number" class="ticker" style="display: inline-block;">{{ fmtInt(displayBlock.number) }}</span>
        </div>
        <div class="mono text-xs dim wh-head-sub">
          <template v-if="!isHovering && head">
            by {{ addrLabel(head.author, head.author_name) }} ·
          </template>
          {{ displayBlock.extrinsic_count }} ext · {{ displayBlock.event_count }} evt
          <template v-if="isHovering"> · {{ displayBlock.ref_time_pct }}% ref</template>
        </div>
      </div>

      <div class="wh-leftlabel">
        <span class="ui-label">— {{ BARS }} blocks ago</span>
      </div>
    </div>

    <!-- Bottom title + CTA bar -->
    <div class="wh-bottom">
      <div class="wh-title">
        <h1>The network, <em>in motion</em><span style="color: var(--red-500);">.</span></h1>
      </div>
      <div class="wh-actions">
        <button type="button" class="btn primary" :disabled="!headWaveform" @click="goToHead">
          Head block
        </button>
        <NuxtLink :to="`/blocks${queryString}`" class="btn">
          All blocks
        </NuxtLink>
        <NuxtLink :to="`/ats${queryString}`" class="btn ghost">
          ATS registry
        </NuxtLink>
      </div>
    </div>
  </div>
</template>

<style scoped>
.waveform-hero {
  --wh-bar-top: #F3EADB;
  --wh-bar-bot: #6D6457;
  --wh-bar-near: var(--teal-500);
  /* Flat dim tone for placeholder bars before real block data arrives.
     Keeps the loading state reading as skeleton instead of a field of
     white tips (gradient stop 0% is near-white in dark mode, so short
     bars would otherwise visually dominate and then "darken" once real
     heights land — jarring). */
  --wh-bar-skel: var(--line-2);
  --wh-axis: var(--line-2);
  position: relative;
  background: var(--bg-elev);
  border: 1px solid var(--line);
  border-radius: 18px;
  overflow: hidden;
  padding: 22px 28px 26px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

@media (prefers-color-scheme: light) {
  .waveform-hero {
    --wh-bar-top: #1A1814;
    --wh-bar-bot: #A89D86;
  }
}

.waveform-hero::before {
  content: '';
  position: absolute;
  inset: 0;
  background:
    radial-gradient(ellipse at 85% 50%, rgba(255, 74, 95, 0.10), transparent 45%),
    radial-gradient(ellipse at 10% 110%, rgba(0, 177, 140, 0.07), transparent 50%);
  pointer-events: none;
}

.wh-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  position: relative;
  z-index: 2;
  flex-wrap: wrap;
  gap: 8px;
}

.wh-meta {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.wh-sep {
  color: var(--ink-dimmer);
  font-family: var(--font-mono);
}

.wh-countdown {
  display: flex;
  align-items: center;
  gap: 8px;
}

.wh-next-num {
  font-size: 13px;
  font-weight: 600;
  color: var(--red-500);
  min-width: 48px;
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.wh-stage {
  position: relative;
  height: 260px;
  border-top: 1px solid var(--line);
  border-bottom: 1px solid var(--line);
  margin: 0 -28px;
  overflow: hidden;
}

.wh-svg {
  width: 100%;
  height: 100%;
  display: block;
}

/* Per-bar hover: subtle uniform zoom on the bar shape itself. The bar
   group contains the top rect + its centerline-mirrored counterpart,
   so the bbox is naturally centred on y = 130 — `transform-box:
   fill-box` with `transform-origin: center` makes the scale anchor
   there and the bar grows outward symmetrically from the axis. */
.wh-bar {
  transform-box: fill-box;
  transform-origin: center;
}

.wh-bar--real {
  cursor: pointer;
  transition: transform 0.2s cubic-bezier(0.2, 0.8, 0.2, 1);
  /* Enter animation — plays once per mount. `backwards` applies the
     `from` keyframe during the stagger delay (so freshly-mounted bars
     stay collapsed at the axis until their turn) and releases control
     back to cascaded rules on finish, leaving `:hover` free to drive
     the hover zoom afterward. Stagger is right-to-left: the head (i =
     BARS-1) enters with 0 ms delay so per-cycle single-bar inserts feel
     instant. `--bar-total` is set reactively from the script so mobile's
     smaller bar count still lands the head at 0 delay without hardcoding. */
  animation: wh-bar-enter 0.55s cubic-bezier(0.34, 1.25, 0.64, 1) backwards;
  animation-delay: calc((var(--bar-total, 72) - 1 - var(--bar-i, 0)) * 4ms);
}

.wh-bar--real:hover {
  transform: scale(1.18);
}

@keyframes wh-bar-enter {
  from {
    opacity: 0;
    transform: scaleY(0);
  }
  to {
    opacity: 1;
    transform: scaleY(1);
  }
}

/* Skeleton layer — always present behind the real bars; opacity binding
   handles the crossfade when data arrives. Sits outside the cycle-scroll
   transform so it doesn't drift while real bars shift over it. */
.wh-skeleton-layer {
  transition: opacity 0.55s ease-out;
  pointer-events: none;
}

/* Skeleton bar breathing. Low-amplitude, slow — reads as "listening"
   rather than "loading". Each bar's animation-delay is offset by its
   slot index so the wave propagates smoothly across the stage; one full
   2.4 s cycle is spread exactly across the current bar count, so the
   wave loops without a visible seam at 72, 48, or 32 bars. */
.wh-skel-bar {
  transform-box: fill-box;
  transform-origin: center;
  animation: wh-skel-breathe 2.4s ease-in-out infinite;
  animation-delay: calc(var(--bar-i, 0) * -2400ms / var(--bar-total, 72));
}

@keyframes wh-skel-breathe {
  0%, 100% {
    transform: scaleY(0.65);
    opacity: 0.45;
  }
  50% {
    transform: scaleY(1.15);
    opacity: 0.8;
  }
}

/* Sweep fades in instead of teleporting from -120 (SSR rest position) to
   its post-mount cyclePos. 600ms mirrors the .reveal fade so the sweep
   settles around the same time the rest of the hero finishes appearing. */
.wh-sweep {
  transition: opacity 0.6s ease-out;
}

@media (prefers-reduced-motion: reduce) {
  .wh-bar--real,
  .wh-skel-bar {
    animation: none;
  }
  .wh-skeleton-layer {
    transition: none;
  }
}

.wh-head {
  position: absolute;
  right: 96px;
  top: 50%;
  /* transform is driven from JS (rAF lerp on currentCardX). The default
     `translate(0px, -50%)` is what the computed `cardTransform` returns
     when nothing's hovered, so SSR + first frame agree. */
  transform: translate(0, -50%);
  text-align: right;
  z-index: 3;
  cursor: pointer;
  padding: 14px 16px;
  background: color-mix(in oklab, var(--bg-elev-solid) 72%, transparent);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
  border: 1px solid var(--line);
  border-radius: 12px;
  min-width: 220px;
  /* Border + background colour fades stay on CSS — only `transform` is
     pinned to the rAF loop, so the two animation systems don't fight. */
  transition:
    border-color 0.25s ease,
    background-color 0.25s ease,
    box-shadow 0.25s ease;
  will-change: transform;
}

.wh-head:hover,
.wh-head--hover {
  border-color: var(--line-2);
  background: color-mix(in oklab, var(--bg-elev-solid) 88%, transparent);
  box-shadow: 0 18px 40px -20px rgba(0, 0, 0, 0.6);
}

.wh-head-label {
  color: var(--red-500);
  transition: color 0.25s ease;
}

.wh-head--hover .wh-head-label {
  color: var(--teal-500);
}

.wh-head-num {
  font-family: var(--font-display);
  font-size: 44px;
  font-weight: 700;
  letter-spacing: -0.035em;
  line-height: 1;
  margin-top: 4px;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

.wh-head-sub {
  margin-top: 8px;
  white-space: nowrap;
}

.wh-leftlabel {
  position: absolute;
  left: 32px;
  bottom: 12px;
  z-index: 2;
  opacity: 0.5;
  pointer-events: none;
}

.wh-bottom {
  display: flex;
  align-items: end;
  justify-content: space-between;
  gap: 24px;
  flex-wrap: wrap;
  position: relative;
  z-index: 2;
  padding-top: 4px;
}

.wh-title h1 {
  font-family: var(--font-display);
  font-size: clamp(32px, 4vw, 52px);
  font-weight: 700;
  letter-spacing: -0.035em;
  line-height: 1;
  margin: 0;
}

.wh-title h1 em {
  font-style: italic;
  font-weight: 400;
  color: var(--ink-dim);
}

.wh-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.wh-actions .btn[disabled] {
  opacity: 0.55;
  cursor: not-allowed;
}

@media (max-width: 900px) {
  .waveform-hero {
    padding: 18px 18px 20px;
    border-radius: 14px;
    gap: 12px;
  }
  .wh-stage {
    height: 200px;
    /* Stage breaks the horizontal padding on desktop (margin: 0 -28px)
       so the waveform hits the card edges. Mirror that for the mobile
       padding value above. */
    margin: 0 -18px;
  }
  .wh-head {
    /* Mobile: hug the edge again — stage real estate is too tight to
       afford the desktop breathing room, and the card mostly stays
       static here anyway. */
    right: 14px;
    min-width: 160px;
    padding: 10px 12px;
  }
  .wh-head-num { font-size: 30px; }
  .wh-leftlabel { display: none; }
}

@media (max-width: 640px) {
  .wh-stage { height: 170px; }
  .wh-top {
    /* Meta + countdown stack because the network name alone can push
       "Runtime N · Finalized #…" past the row width. */
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
  }
  .wh-countdown {
    align-self: flex-end;
  }
  .wh-bottom {
    /* Title on its own line, then the CTA row underneath. flex-wrap
       alone tends to leave the buttons half-width; splitting the axis
       gives each block full width and stacks cleanly. */
    flex-direction: column;
    align-items: stretch;
    gap: 10px;
  }
  .wh-title h1 { font-size: 24px; }
  .wh-actions {
    width: 100%;
  }
  .wh-actions .btn {
    flex: 1;
    min-height: 38px;
    justify-content: center;
  }
  .wh-head {
    right: 10px;
    min-width: 0;
    max-width: 60%;
    padding: 8px 10px;
  }
  .wh-head-num { font-size: 24px; }
  .wh-head-sub {
    font-size: 10.5px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
}

@media (max-width: 480px) {
  .waveform-hero {
    padding: 14px 14px 18px;
  }
  .wh-stage {
    height: 150px;
    margin: 0 -14px;
  }
}
</style>

<script setup lang="ts">
// "Protect your work" CTA — button + transition modal. Used from the
// ATS hero and the dashboard promo panel. The actual workflow lives at
// app.allfeat.org, so the modal is a short bridge that announces the
// hand-off before opening the external surface in a new tab.

import { nextTick, ref, watch } from 'vue'
import { onKeyStroke, useScrollLock } from '@vueuse/core'

const props = withDefaults(defineProps<{
  /** Visual weight of the trigger button. */
  variant?: 'primary' | 'ghost'
  /** Optional override for the CTA label. */
  label?: string
}>(), {
  variant: 'primary',
  label: 'Protect your work',
})

const PROTECT_URL = 'https://app.allfeat.org'

const open = ref(false)
const continueBtn = ref<HTMLElement | null>(null)

// Lock background scroll + move focus into the dialog while it's open.
// useScrollLock is a no-op on SSR (document is the probe).
const locked = useScrollLock(typeof document !== 'undefined' ? document.body : null)
watch(open, (isOpen) => {
  locked.value = isOpen
  if (isOpen) nextTick(() => continueBtn.value?.focus())
})

onKeyStroke('Escape', () => {
  if (open.value) open.value = false
})

function show() {
  open.value = true
}

function dismiss() {
  open.value = false
}

function proceed() {
  // `noopener,noreferrer` so the external tab can't reach back into
  // window.opener — standard hardening for any outbound link.
  window.open(PROTECT_URL, '_blank', 'noopener,noreferrer')
  open.value = false
}
</script>

<template>
  <button
    type="button"
    class="btn protect-cta"
    :class="[variant]"
    @click="show"
  >
    <span>{{ props.label }}</span>
    <svg
      class="protect-cta-arrow"
      width="16" height="16" viewBox="0 0 24 24"
      fill="none" stroke="currentColor" stroke-width="2"
      stroke-linecap="round" stroke-linejoin="round"
      aria-hidden="true"
    >
      <polyline points="9 18 15 12 9 6" />
    </svg>
  </button>

  <Teleport to="body">
    <Transition name="protect-modal">
      <div
        v-if="open"
        class="protect-root"
        role="dialog"
        aria-modal="true"
        aria-labelledby="protect-title"
      >
        <button
          type="button"
          class="protect-backdrop"
          aria-label="Close"
          @click="dismiss"
        />

        <div class="protect-panel panel">
          <div class="ui-label protect-kicker">Allfeat Timestamp</div>
          <h3 id="protect-title" class="protect-title">
            You're leaving the Explorer
          </h3>
          <p class="protect-lede">
            The Protection tool lives at
            <span class="mono protect-url">app.allfeat.org</span>.
            Start by creating your Allfeat account, then protect your
            works through the ATS feature.
          </p>
          <div class="protect-actions">
            <button
              type="button"
              class="btn ghost"
              @click="dismiss"
            >
              Cancel
            </button>
            <button
              ref="continueBtn"
              type="button"
              class="btn primary protect-continue"
              @click="proceed"
            >
              <span>Continue</span>
              <svg
                width="16" height="16" viewBox="0 0 24 24"
                fill="none" stroke="currentColor" stroke-width="2"
                stroke-linecap="round" stroke-linejoin="round"
                aria-hidden="true"
              >
                <path d="M7 17 17 7M9 7h8v8" />
              </svg>
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped lang="scss">
.protect-cta {
  display: inline-flex;
  align-items: center;
  gap: 6px;

  .protect-cta-arrow {
    transition: transform 0.2s cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  &:hover .protect-cta-arrow {
    transform: translateX(3px);
  }
}

.protect-root {
  position: fixed;
  inset: 0;
  z-index: 300;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
  pointer-events: none;

  > * { pointer-events: auto; }
}

.protect-backdrop {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
  border: 0;
  padding: 0;
  cursor: pointer;
}

[data-theme="light"] .protect-backdrop {
  background: rgba(21, 21, 21, 0.4);
}

.protect-panel {
  position: relative;
  width: min(440px, 100%);
  padding: 28px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.protect-kicker {
  color: var(--teal-500);
}

.protect-title {
  font-family: var(--font-display);
  font-size: 24px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1.15;
}

.protect-lede {
  color: var(--ink-dim);
  font-size: 14px;
  line-height: 1.55;
}

.protect-url {
  color: var(--ink);
  font-size: 13px;
}

.protect-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
  margin-top: 6px;
}

.protect-continue {
  display: inline-flex;
  align-items: center;
  gap: 6px;

  svg {
    transition: transform 0.2s cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  &:hover svg {
    transform: translate(2px, -2px);
  }
}

// ——— transitions ———

.protect-modal-enter-active .protect-panel,
.protect-modal-leave-active .protect-panel {
  transition:
    transform 0.24s cubic-bezier(0.2, 0.8, 0.2, 1),
    opacity 0.2s ease;
}
.protect-modal-enter-active .protect-backdrop,
.protect-modal-leave-active .protect-backdrop {
  transition: opacity 0.2s ease;
}

.protect-modal-enter-from .protect-panel,
.protect-modal-leave-to .protect-panel {
  opacity: 0;
  transform: translateY(8px) scale(0.98);
}
.protect-modal-enter-from .protect-backdrop,
.protect-modal-leave-to .protect-backdrop {
  opacity: 0;
}

@media (max-width: 640px) {
  .protect-panel {
    padding: 22px;
  }
  .protect-title {
    font-size: 20px;
  }
  .protect-actions {
    flex-direction: column-reverse;

    .btn {
      width: 100%;
      justify-content: center;
    }
  }
}

@media (prefers-reduced-motion: reduce) {
  .protect-modal-enter-active .protect-panel,
  .protect-modal-leave-active .protect-panel,
  .protect-cta .protect-cta-arrow,
  .protect-continue svg {
    transition: none;
  }
  .protect-cta:hover .protect-cta-arrow,
  .protect-continue:hover svg {
    transform: none;
  }
}
</style>

<script setup lang="ts">
// Generic 404 panel shared by every detail route (/blocks/:number,
// /accounts/:address, /extrinsics/:id, /ats/:id). Before extraction
// each page hand-rolled its own copy of this markup.

interface Props {
  /** Title-case entity label, e.g. "Block", "Account". */
  entity: string
  /** Identifier to show — a formatted number, address, or id. */
  name: string
  /** Network display name ("Melodie"), or null when unknown. */
  networkName?: string | null
  /** Route to return to. */
  backTo: string
  /** Link copy. */
  backLabel: string
}

defineProps<Props>()
</script>

<template>
  <div class="panel not-found">
    <div class="not-found__body">
      <div class="not-found__tag">404 — Not found</div>
      <h2 class="not-found__title">
        {{ entity }} {{ name }}
      </h2>
      <p class="not-found__desc">
        No {{ entity.toLowerCase() }} with this identifier was found on
        {{ networkName ?? 'this network' }}.
      </p>
      <NuxtLink :to="backTo" class="btn">{{ backLabel }}</NuxtLink>
    </div>
  </div>
</template>

<style scoped lang="scss">
.not-found {
  margin-top: 40px;

  &__body {
    padding: 60px 40px;
    text-align: center;
  }

  &__tag {
    font-family: var(--font-mono);
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--red-500);
    margin-bottom: 12px;
  }

  &__title {
    font-family: var(--font-display);
    font-size: 28px;
    margin-bottom: 10px;
  }

  &__desc {
    color: var(--ink-dim);
    margin-bottom: 24px;
  }
}
</style>

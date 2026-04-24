<script setup lang="ts">
// Dashboard promo card — pitches the ATS protocol and links into the
// registry. Shows the current total-timestamped count when available.

defineProps<{
  totalTimestamped?: number | null
}>()

const { queryString } = useNetworkLink()
</script>

<template>
  <div class="panel ats-promo">
    <div>
      <div class="ui-label ats-promo-kicker">Allfeat Timestamp · v1</div>
      <h2 class="ats-promo-title">
        Every work on Allfeat<br>
        gets a <span class="ats-promo-highlight">heartbeat</span><br>
        from a <span class="ats-promo-highlight">provable origin</span>.
      </h2>
      <p v-if="totalTimestamped != null" class="ats-promo-lede">
        {{ fmtInt(totalTimestamped) }} works timestamped · solid hashing algorithm
        designed to prove authorship of every musical work.
      </p>
    </div>
    <div class="ats-promo-actions">
      <AtsProtectCta variant="primary" />
      <NuxtLink :to="`/ats${queryString}`" class="btn ghost">Explore ATS</NuxtLink>
      <a
        href="https://github.com/Allfeat/ats-sdk"
        target="_blank"
        rel="noopener noreferrer"
        class="btn ghost"
      >Protocol SDK</a>
    </div>
  </div>
</template>

<style scoped lang="scss">
.ats-promo {
  padding: 28px;
  background: linear-gradient(135deg, rgba(0, 177, 140, 0.08) 0%, transparent 70%);
  display: flex;
  flex-direction: column;
  justify-content: space-between;
}

.ats-promo-kicker {
  color: var(--teal-500);
  margin-bottom: 14px;
}

.ats-promo-title {
  font-family: var(--font-display);
  font-size: 32px;
  letter-spacing: -0.025em;
  line-height: 1.05;
  font-weight: 700;
  text-wrap: balance;
  margin-bottom: 14px;
}

.ats-promo-highlight {
  color: var(--teal-500);
}

.ats-promo-lede {
  color: var(--ink-dim);
  font-size: 14px;
  line-height: 1.55;
  max-width: 440px;
}

.ats-promo-actions {
  display: flex;
  gap: 8px;
  margin-top: 24px;
}

.ats-promo-actions .btn[disabled] {
  opacity: 0.55;
  cursor: not-allowed;
}

@media (max-width: 640px) {
  .ats-promo {
    padding: 20px;
  }
  .ats-promo-title {
    font-size: 26px;
  }
  .ats-promo-actions .btn {
    flex: 1;
    min-height: 40px;
    justify-content: center;
  }
}
</style>

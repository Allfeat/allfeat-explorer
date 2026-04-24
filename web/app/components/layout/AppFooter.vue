<script setup lang="ts">
// Footer shell — four informational columns + a bottom strip carrying
// the build stamp and the live head/finalized readout.
//
// The accent-line div sits above the footer for future re-enablement;
// _footer.scss currently hides it but the markup stays in place so we
// don't have to re-thread it through every page later.

import { storeToRefs } from 'pinia'
import { useNetworksStore } from '~/stores/networks'
import { useLiveStore } from '~/stores/live'
import { useConnectionState } from '~/composables/useConnectionState'
import { useBuildInfo } from '~/composables/useBuildInfo'

const networks = useNetworksStore()
const live = useLiveStore()
const { head, finalizedHead } = storeToRefs(live)
const connection = useConnectionState()

const { info: buildInfo } = useBuildInfo()
const YEAR = new Date().getFullYear()

// Back-fill the API half with "…" until the `/meta` fetch resolves, so
// the strip stays the same width between SSR and hydration. `unknown`
// shows up when a backend was built outside a git checkout (Docker
// images without the repo mounted) — rendering it verbatim is better
// than hiding the gap.
const frontendStamp = computed(() => {
  const f = buildInfo.value.frontend
  return `${f.version} · ${f.gitSha}`
})
const apiStamp = computed(() => {
  const a = buildInfo.value.api
  if (!a.version || !a.gitSha) return '…'
  return `${a.version} · ${a.gitSha}`
})
</script>

<template>
  <div class="accent-line" />
  <footer class="footer">
    <div class="container footer-inner">
      <div class="footer-col" style="flex: 1;">
        <div class="footer-brand">
          <BrandLogo class="brand-mark" />
          <span class="brand-suffix">Explorer</span>
        </div>
        <div style="font-size: 11px;">Substrate-based chain explorer · Ref-impl</div>
      </div>

      <div class="footer-col">
        <div class="ui-label">Networks</div>
        <a
          v-for="n in networks.items"
          :key="n.id"
          :href="`/?network=${n.id}`"
        >{{ n.name }} — {{ n.kind }}</a>
      </div>

      <div class="footer-col">
        <div class="ui-label">Developers</div>
        <a href="#">RPC endpoints</a>
        <a href="#">Runtime metadata</a>
        <a href="#">GitHub</a>
      </div>

      <div class="footer-col">
        <div class="ui-label">Resources</div>
        <a href="#">Docs</a>
        <a href="#">Status</a>
        <a href="#">Brand kit</a>
      </div>
    </div>

    <div class="container footer-bottom">
      <span class="mono footer-build">
        © {{ YEAR }} ALLFEAT FOUNDATION · WEB {{ frontendStamp }} · API {{ apiStamp }}
      </span>
      <span class="footer-live">
        <ClientOnly>
          <ConnectionIndicator :state="connection" />
          <template #fallback>
            <ConnectionIndicator state="connecting" />
          </template>
        </ClientOnly>
        <FooterHeadChip :head="head" :finalized="finalizedHead" />
      </span>
    </div>
  </footer>
</template>

<style scoped lang="scss">
.footer-brand {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 8px;
}

.footer-bottom {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 16px;
  margin-top: 32px;
  font-size: 11px;
  color: var(--ink-muted);
  flex-wrap: wrap;
}

.footer-build {
  letter-spacing: 0.04em;
}

.footer-live {
  display: inline-flex;
  align-items: center;
  gap: 12px;
}

@media (max-width: 640px) {
  .footer-bottom {
    margin-top: 22px;
    gap: 12px;
    justify-content: flex-start;
  }

  .footer-live {
    gap: 10px;
    flex-wrap: wrap;
  }
}
</style>

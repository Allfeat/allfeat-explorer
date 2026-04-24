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

const networks = useNetworksStore()
const live = useLiveStore()
const { head, finalizedHead } = storeToRefs(live)
const connection = useConnectionState()

const BUILD_LABEL = '1.0.0-dev'
const YEAR = new Date().getFullYear()
</script>

<template>
  <div class="accent-line" />
  <footer class="footer">
    <div class="container footer-inner">
      <div class="footer-col" style="flex: 1;">
        <div style="display: flex; align-items: center; gap: 10px; margin-bottom: 8px;">
          <span class="brand-word">Allfeat<span class="dot">.</span></span>
        </div>
        <div style="font-size: 11px;">Substrate-based chain explorer · Ref-impl</div>
        <div style="font-size: 11px; margin-top: 4px;">Built for the living network.</div>
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
      <span class="mono footer-build">© {{ YEAR }} ALLFEAT FOUNDATION · BUILD {{ BUILD_LABEL }}</span>
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

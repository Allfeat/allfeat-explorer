<script setup lang="ts">
// ATS detail page — /ats/:id where :id is the ats_id. The file is named
// `[id].vue` (not `[index].vue`) because the literal string "index"
// collides with the sibling `index.vue` route in Nuxt's naming scheme.

import type { AtsRecord } from '@bindings'

const route = useRoute()
const atsId = computed<string>(() => String(route.params.id ?? ''))

const { spec } = useActiveNetwork()

const { data: record, error } = useNetworkFetch<AtsRecord | null>(
  net => `ats-record:${net}:${atsId.value}`,
  net => `/networks/${net}/ats/${atsId.value}`,
  { default: () => null, watch: [atsId] },
)

const breadcrumb = computed(() => [
  { label: 'Home', to: '/' },
  { label: 'ATS', to: '/ats' },
  { label: `#${atsId.value}` },
])

definePageMeta({ name: 'ats-detail' })
useSeoMeta({
  title: () => record.value
    ? `ATS #${record.value.ats_id} · Allfeat Explorer`
    : 'ATS · Allfeat Explorer',
  description: () => record.value
    ? `Allfeat Timestamp #${record.value.ats_id} on ${spec.value?.name ?? 'Allfeat'}.`
    : 'ATS record on Allfeat.',
  ogType: 'article',
})
</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <LazyAtsDetail v-if="record" :record="record" />

    <NotFoundPanel
      v-else-if="error"
      entity="ATS"
      :name="`#${atsId}`"
      :network-name="spec?.name"
      back-to="/ats"
      back-label="Back to ATS"
    />

    <div v-else class="panel" style="margin-top: 40px;">
      <div class="panel-body">
        <SkeletonRows :rows="6" :columns="['160px', '1fr']" />
      </div>
    </div>
  </section>
</template>

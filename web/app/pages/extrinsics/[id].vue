<script setup lang="ts">
// Extrinsic detail page — /extrinsics/:id where :id is <block>-<index>.
// Fetches the single extrinsic and delegates rendering to
// <ExtrinsicDetail/>.

import type { Extrinsic } from '@bindings'

const route = useRoute()
const id = computed<string>(() => String(route.params.id ?? ''))

const { spec } = useActiveNetwork()

const { data: extrinsic, error } = useNetworkFetch<Extrinsic | null>(
  net => `extrinsic-detail:${net}:${id.value}`,
  net => `/networks/${net}/extrinsics/${id.value}`,
  { default: () => null, watch: [id] },
)

const breadcrumb = computed(() => [
  { label: 'Home', to: '/' },
  { label: 'Extrinsics', to: '/extrinsics' },
  { label: id.value },
])

definePageMeta({ name: 'extrinsic-detail' })
useSeoMeta({
  title: () => extrinsic.value
    ? `Extrinsic ${extrinsic.value.id} · Allfeat Explorer`
    : 'Extrinsic · Allfeat Explorer',
  description: () => extrinsic.value
    ? `Extrinsic ${extrinsic.value.id} on ${spec.value?.name ?? 'Allfeat'} — ${extrinsic.value.module}.${extrinsic.value.call}.`
    : 'Extrinsic details on Allfeat.',
  ogType: 'article',
})
</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <LazyExtrinsicDetail v-if="extrinsic" :extrinsic="extrinsic" />

    <NotFoundPanel
      v-else-if="error"
      entity="Extrinsic"
      :name="id"
      :network-name="spec?.name"
      back-to="/extrinsics"
      back-label="Back to extrinsics"
    />

    <div v-else class="panel" style="margin-top: 40px;">
      <div class="panel-body">
        <SkeletonRows :rows="6" :columns="['160px', '1fr']" />
      </div>
    </div>
  </section>
</template>

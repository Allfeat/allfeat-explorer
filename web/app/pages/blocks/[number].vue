<script setup lang="ts">
// Block detail page — /blocks/:number. Fetches the block + its extrinsics
// in parallel on SSR. Rendering is delegated to <BlockDetail/>; this file
// is the route handler (params, fetches, 404 fallback).

import type { Block, BlockEvent, Extrinsic } from '@bindings'

const route = useRoute()
const number = computed<string>(() => String(route.params.number ?? ''))

const { spec } = useActiveNetwork()

const { data: block, error: blockError } = useNetworkFetch<Block | null>(
  net => `block-detail:${net}:${number.value}`,
  net => `/networks/${net}/blocks/${number.value}`,
  { default: () => null, watch: [number] },
)

const { data: extrinsics } = useNetworkFetch<Extrinsic[]>(
  net => `block-extrinsics:${net}:${number.value}`,
  net => `/networks/${net}/blocks/${number.value}/extrinsics`,
  { default: (): Extrinsic[] => [], watch: [number] },
)

const { data: events } = useNetworkFetch<BlockEvent[]>(
  net => `block-events:${net}:${number.value}`,
  net => `/networks/${net}/blocks/${number.value}/events`,
  { default: (): BlockEvent[] => [], watch: [number] },
)

const breadcrumb = computed(() => [
  { label: 'Home', to: '/' },
  { label: 'Blocks', to: '/blocks' },
  { label: `#${fmtInt(number.value)}` },
])

definePageMeta({ name: 'block-detail' })
useSeoMeta({
  title: () => block.value
    ? `Block #${fmtInt(block.value.number)} · Allfeat Explorer`
    : 'Block · Allfeat Explorer',
  description: () => block.value
    ? `Block #${fmtInt(block.value.number)} on ${spec.value?.name ?? 'Allfeat'} — ${block.value.extrinsic_count} extrinsics, ${block.value.event_count} events.`
    : 'Block details on Allfeat.',
  ogType: 'article',
})
</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <LazyBlockDetail
      v-if="block"
      :block="block"
      :extrinsics="extrinsics"
      :events="events"
      :network="spec"
    />

    <NotFoundPanel
      v-else-if="blockError"
      entity="Block"
      :name="`#${fmtInt(number)}`"
      :network-name="spec?.name"
      back-to="/blocks"
      back-label="Back to blocks"
    />

    <div v-else class="panel" style="margin-top: 40px;">
      <div class="panel-body">
        <SkeletonRows :rows="6" :columns="['160px', '1fr']" />
      </div>
    </div>
  </section>
</template>

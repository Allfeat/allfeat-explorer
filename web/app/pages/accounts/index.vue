<script setup lang="ts">
// Accounts list — top holders by native-token balance. The API returns a
// ranked `Page<Account>` slice (top-N, no cursor pagination yet); we
// render `page_info.items` verbatim.

import type { Account, Page } from '@bindings'

definePageMeta({ name: 'accounts-list' })
useSeoMeta({
  title: 'Accounts · Allfeat Explorer',
  description: 'Top accounts on Allfeat by native-token balance.',
  ogTitle: 'Accounts · Allfeat',
  ogDescription: 'Top holders on the Allfeat chain.',
  ogType: 'website',
})

const COUNT = 20

const { data: page, pending, error } = useNetworkFetch<Page<Account>>(
  net => `accounts-list:${net}`,
  net => `/networks/${net}/accounts?count=${COUNT}`,
  {
    default: (): Page<Account> => ({
      items: [],
      page_info: { total: null, next_cursor: null, has_more: false },
    }),
  },
)

const accounts = computed<Account[]>(() => page.value?.items ?? [])

const breadcrumb = [
  { label: 'Home', to: '/' },
  { label: 'Accounts' },
]

const skeletonCols = ['60px', '1fr', '200px', '120px', '140px']
</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <div class="page-title">
      <div>
        <h1>Accounts</h1>
      </div>
    </div>

    <div class="panel" style="margin-top: 20px;">
      <AccountsTable v-if="accounts.length > 0" :accounts="accounts" />
      <SkeletonRows v-else-if="pending" :rows="10" :columns="skeletonCols" />
      <p v-else-if="error" class="dim" style="padding: 40px; text-align: center;">
        Failed to load accounts.
      </p>
      <p v-else class="dim" style="padding: 40px; text-align: center;">
        No accounts found.
      </p>
    </div>
  </section>
</template>

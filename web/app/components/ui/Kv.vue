<script setup lang="ts">
// Grid container for key/value rows on detail pages. Accepts <KvRow/>
// children, or raw `<div class="k"/>` + `<div class="v"/>` pairs if the
// consumer needs full control.
</script>

<template>
  <div class="kv">
    <slot />
  </div>
</template>

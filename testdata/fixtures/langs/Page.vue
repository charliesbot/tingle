<script setup lang="ts">
import Badge from './Badge.vue'
import { ref } from 'vue'

const title = ref('Hello')
</script>

<template>
  <section>
    <h1>{{ title }}</h1>
    <Badge label="new" />
    <Callout tone="warn">Auto-registered component, no explicit import.</Callout>
  </section>
</template>

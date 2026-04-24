<script setup lang="ts">
// ATS detail → Technical tab. Static description of the hashing
// pipeline, the canonical encoding rules, and the on-chain footprint.
// Only the on-chain commitment hash changes per selected version, so
// this tab's dynamic surface is minimal.

import type { AtsVersion } from '@bindings'
import { shortHash } from '~/utils/format'

defineProps<{
  selected: AtsVersion
}>()
</script>

<template>
  <div class="technical-stack">
    <div class="panel">
      <div class="panel-head">
        <h3>Hashing pipeline</h3>
        <span class="tag">ats-sdk · commitment.rs</span>
      </div>
      <div class="pipeline">
        <div class="pipeline-row">
          <div class="step-num">1</div>
          <div class="step-line">
            <div class="step-line__head">
              <span class="ui-label">media_hash</span>
              <span class="step-formula">= SHA-256(file_bytes)</span>
            </div>
            <div class="mono text-xs dim">Hash of the raw audio / media file, computed by the uploader</div>
          </div>
        </div>
        <div class="pipeline-row">
          <div class="step-num">2</div>
          <div class="step-line">
            <div class="step-line__head">
              <span class="ui-label">creator_leaves</span>
              <span class="step-formula">= SHA-256(canonical_encode(creator[i]))</span>
            </div>
            <div class="mono text-xs dim">Each creator encoded deterministically — full_name, email, roles, ipi, isni</div>
          </div>
        </div>
        <div class="pipeline-row">
          <div class="step-num">3</div>
          <div class="step-line">
            <div class="step-line__head">
              <span class="ui-label">merkle_root</span>
              <span class="step-formula">= MerkleTree(leaves, depth=5, pad=SHA-256(0x00))</span>
            </div>
            <div class="mono text-xs dim">Fixed depth 5 → up to 32 creators · pad with SHA-256(0x00)</div>
          </div>
        </div>
        <div class="pipeline-row">
          <div class="step-num">4</div>
          <div class="step-line">
            <div class="step-line__head">
              <span class="ui-label step-line__head--accent">commitment</span>
              <span class="step-formula step-formula--accent">= SHA-256(version ∥ media_hash ∥ canonical(title) ∥ merkle_root)</span>
            </div>
            <div class="mono text-xs dim">Single 32-byte hash submitted on-chain. Everything else stays off-chain.</div>
          </div>
        </div>
      </div>
    </div>

    <div class="panel">
      <div class="panel-head">
        <h3>Canonical encoding</h3>
        <span class="tag">canonical.rs · deterministic · language-agnostic</span>
      </div>
      <div class="canonical">
        <div>
          <div class="ui-label" style="margin-bottom: 8px;">String</div>
          <div class="codeline">[length : u32 LE] [UTF-8 bytes]</div>
          <div class="ui-label" style="margin-top: 14px; margin-bottom: 8px;">Option&lt;String&gt;</div>
          <div class="codeline">0x00 // None</div>
          <div class="codeline">0x01 || string_encoding // Some</div>
        </div>
        <div>
          <div class="ui-label" style="margin-bottom: 8px;">Roles</div>
          <div class="codeline">[count : u8] [tag_bytes...]</div>
          <div class="mono text-xs dim" style="margin-top: 6px;">deduplicated · sorted by tag ascending</div>
          <div class="ui-label" style="margin-top: 14px; margin-bottom: 8px;">Creator field order</div>
          <div class="codeline">full_name → email → roles → ipi → isni</div>
        </div>
      </div>
    </div>

    <div class="technical-grid">
      <div class="panel">
        <div class="panel-head">
          <h3>On-chain footprint</h3>
          <span class="tag">pallet-ats</span>
        </div>
        <Kv>
          <KvRow label="commitment">
            <span class="mono" style="word-break: break-all; color: var(--teal-500);">
              H256 · {{ selected.commitment }}
            </span>
          </KvRow>
          <KvRow label="protocol_version">
            <span class="mono">u8 · {{ selected.protocol_version }}</span>
          </KvRow>
        </Kv>
        <div class="footprint-note">
          Total on-chain footprint: <strong>33 bytes</strong> per registration (32-byte hash +
          1-byte version). Media, title, and creators live exclusively in the off-chain certificate.
        </div>
      </div>

      <div class="panel">
        <div class="panel-head">
          <h3>Verification</h3>
          <span class="tag">ats-sdk · verify_commitment</span>
        </div>
        <div class="verify-body">
          <div class="verify-code mono text-xs">
            <div class="dim">// Given the off-chain inputs + media</div>
            <div>let proof = generate_commitment(&amp;input, media)?;</div>
            <div>assert_eq!(</div>
            <div style="padding-left: 14px;">proof.on_chain.commitment,</div>
            <div style="padding-left: 14px; color: var(--teal-500);">
              {{ shortHash(selected.commitment, 10, 8) }}
            </div>
            <div>);</div>
          </div>
          <div class="mono text-xs dim" style="margin-top: 12px; line-height: 1.6;">
            Any conforming implementation (Rust, TypeScript, Python…) producing the same inputs
            must yield this exact commitment hash.
          </div>
        </div>
      </div>
    </div>

    <div class="panel">
      <div class="panel-head">
        <h3>Protocol limits</h3>
      </div>
      <div class="limits-grid">
        <div class="limit-cell">
          <div class="ui-label">Creators per work</div>
          <div class="limit-val">1 – 32</div>
          <div class="mono text-xs dim">Merkle depth = 5</div>
        </div>
        <div class="limit-cell">
          <div class="ui-label">Media file</div>
          <div class="limit-val">Any type</div>
          <div class="mono text-xs dim">Must be non-empty</div>
        </div>
        <div class="limit-cell">
          <div class="ui-label">Title</div>
          <div class="limit-val">UTF-8</div>
          <div class="mono text-xs dim">Non-empty string</div>
        </div>
        <div class="limit-cell limit-cell--last">
          <div class="ui-label">IPI / ISNI</div>
          <div class="limit-val">Optional</div>
          <div class="mono text-xs dim">11 digits / 16 [0-9X]</div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.technical-stack {
  display: flex;
  flex-direction: column;
  gap: 20px;
}

.pipeline {
  padding: 20px 22px;
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 20px;
  align-items: start;
}

.pipeline-row {
  display: contents;
}

@media (max-width: 640px) {
  .pipeline {
    padding: 16px 14px;
    gap: 14px;
    grid-template-columns: auto 1fr;
  }
}

.step-num {
  width: 28px;
  height: 28px;
  border-radius: 50%;
  border: 1px solid var(--line-2);
  background: var(--bg-elev);
  display: flex;
  align-items: center;
  justify-content: center;
  font-family: var(--font-mono);
  font-size: 12px;
  font-weight: 600;
  color: var(--ink-dim);
}

.step-line {
  min-width: 0;
}

.step-line__head {
  display: flex;
  align-items: baseline;
  flex-wrap: wrap;
  column-gap: 10px;
  row-gap: 2px;
  margin-bottom: 6px;
}

.step-line__head--accent {
  color: var(--teal-500);
}

.step-formula {
  font-family: var(--font-mono);
  font-size: 13px;
  font-weight: 500;
  color: var(--ink-dim);
  word-break: break-word;
  min-width: 0;
}

.step-formula--accent {
  color: var(--ink);
}

.canonical {
  padding: 18px 22px;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 20px;
}

@media (max-width: 720px) {
  .canonical {
    grid-template-columns: 1fr;
  }
}

@media (max-width: 640px) {
  .canonical {
    padding: 16px 14px;
  }
}

.codeline {
  padding: 8px 10px;
  background: var(--chip-bg);
  border: 1px solid var(--chip-bd);
  border-radius: 4px;
  margin-bottom: 4px;
  word-break: break-all;
  font-family: var(--font-mono);
  font-size: 12px;
}

.technical-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 20px;
}

@media (max-width: 960px) {
  .technical-grid {
    grid-template-columns: 1fr;
  }
}

.footprint-note {
  padding: 14px 22px 20px;
  color: var(--ink-dim);
  font-size: 12px;
  line-height: 1.6;
}

.footprint-note strong {
  color: var(--ink);
}

.verify-body {
  padding: 20px 22px;
}

@media (max-width: 640px) {
  .verify-body {
    padding: 16px 14px;
  }
}

.verify-code {
  padding: 14px;
  background: var(--chip-bg);
  border: 1px solid var(--chip-bd);
  border-radius: 6px;
  line-height: 1.7;
  overflow-x: auto;
}

.limits-grid {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
}

@media (max-width: 720px) {
  .limits-grid {
    grid-template-columns: repeat(2, 1fr);
  }
}

.limit-cell {
  padding: 18px 22px;
  border-right: 1px solid var(--line);
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.limit-cell--last {
  border-right: none;
}

@media (max-width: 720px) {
  .limit-cell:nth-child(2n) {
    border-right: none;
  }

  .limit-cell:nth-child(-n+2) {
    border-bottom: 1px solid var(--line);
  }
}

.limit-val {
  font-family: var(--font-display);
  font-size: 18px;
  font-weight: 600;
  letter-spacing: -0.01em;
}
</style>

<script setup lang="ts">
import {nextTick, ref, watch} from 'vue'
import {ListTree, Pencil, Play} from '@lucide/vue'
import {useI18n} from 'vue-i18n'
import type {Chapter} from '../api'
import {chapterTime} from '../chapters'

const props = defineProps<{
    chapters: Chapter[]
    active: number
    duration: number
    editable: boolean
    loading: boolean
}>()
defineEmits<{select: [chapter: Chapter]; edit: [chapter: Chapter]}>()
const {t} = useI18n()
const list = ref<HTMLOListElement>()

watch(() => [props.active, props.chapters], async () => {
    await nextTick()
    const container = list.value
    const item = container?.querySelector<HTMLElement>('.current-chapter')
    if (!container || !item) return
    const bounds = container.getBoundingClientRect()
    const row = item.getBoundingClientRect()
    if (row.top < bounds.top) container.scrollTop += row.top - bounds.top - 6
    else if (row.bottom > bounds.bottom) container.scrollTop += row.bottom - bounds.bottom + 6
}, {immediate: true, flush: 'post'})
</script>

<template>
    <ol v-if="chapters.length" ref="list" class="chapter-list" :aria-label="t('chapters')">
        <li v-for="(chapter, index) in chapters" :key="chapter.startMs" class="chapter-row"
            :class="{'current-chapter': index === active}">
            <button class="chapter-jump" type="button" :disabled="chapter.startMs / 1000 >= duration"
                :aria-current="index === active ? 'location' : undefined" @click="$emit('select', chapter)">
                <span class="chapter-stamp">
                    <Play v-if="index === active" :size="11" fill="currentColor" aria-hidden="true" />
                    {{ chapterTime(chapter.startMs) }}
                </span>
                <span class="chapter-name">{{ chapter.title || t('chapterNumber', {number: index + 1}) }}</span>
            </button>
            <button v-if="editable" type="button" class="chapter-edit" :aria-label="t('chapterEditNamed', {name: chapter.title || t('chapterNumber', {number: index + 1})})"
                :title="t('chapterEdit')" @click="$emit('edit', chapter)">
                <Pencil :size="15" />
            </button>
        </li>
    </ol>
    <div v-else class="chapter-empty" :role="loading ? 'status' : undefined">
        <ListTree :size="24" aria-hidden="true" />
        <strong>{{ t(loading ? 'chapterLoading' : 'chapterEmpty') }}</strong>
        <p v-if="!loading && editable">{{ t('chapterEmptyHelp') }}</p>
    </div>
</template>

<style scoped>
.chapter-list { min-height: 0; overflow: auto; overscroll-behavior: contain; margin: 0; padding: 7px; list-style: none; scrollbar-width: thin; }
.chapter-row { display: flex; align-items: center; min-width: 0; border: 1px solid transparent; border-radius: 8px; }
.chapter-row + .chapter-row { margin-top: 3px; }
.chapter-row:hover { background: var(--hover); }
.chapter-row.current-chapter { border-color: #ff796b4d; background: var(--accent-soft); }
.chapter-jump, .chapter-edit { border: 0; background: none; color: var(--text); border-radius: 6px; }
.chapter-jump { display: flex; align-items: center; gap: 11px; flex: 1; min-width: 0; min-height: 46px; padding: 8px 9px; text-align: start; }
.chapter-stamp { display: inline-flex; align-items: center; gap: 4px; min-width: 45px; color: var(--muted); font-size: 11px; font-variant-numeric: tabular-nums; white-space: nowrap; }
.chapter-name { min-width: 0; overflow-wrap: anywhere; font-size: 13px; line-height: 1.4; font-weight: 620; }
.current-chapter .chapter-stamp, .current-chapter .chapter-name { color: var(--accent); }
.chapter-edit { display: grid; place-items: center; width: 40px; height: 44px; margin-right: 2px; color: var(--muted); }
.chapter-edit:hover { background: var(--raised); color: var(--text); }
.chapter-empty { display: grid; justify-items: center; align-content: center; min-height: 137px; gap: 9px; padding: 16px; text-align: center; }
.chapter-empty svg { color: var(--accent); }
.chapter-empty strong { font-size: 14px; }
.chapter-empty p { max-width: 30ch; color: var(--muted); font-size: 12px; line-height: 1.55; }
</style>

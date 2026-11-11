<script setup lang="ts">
import {Eye, Globe2, Grid2X2, List} from '@lucide/vue'
import {useI18n} from 'vue-i18n'
import {preferences} from '../preferences'
const {t} = useI18n()
</script>

<template>
    <section class="settings-page">
        <h1>{{ t('settings') }}</h1>
        <div class="settings-panel">
            <label class="settings-row">
                <span><Globe2 :size="20" />{{ t('language') }}</span>
                <select v-model="preferences.language" class="select">
                    <option value="zh" lang="zh-CN">简体中文</option>
                    <option value="en" lang="en">English</option>
                </select>
            </label>
            <label class="settings-row">
                <span><component :is="preferences.view === 'grid' ? Grid2X2 : List" :size="20" />{{ t('view') }}</span>
                <select v-model="preferences.view" class="select">
                    <option value="grid">{{ t('grid') }}</option>
                    <option value="list">{{ t('list') }}</option>
                </select>
            </label>
            <label class="settings-row">
                <span><Eye :size="20" />{{ t('showHidden') }}</span>
                <input v-model="preferences.showHidden" type="checkbox">
            </label>
        </div>
    </section>
</template>

<style scoped>
.settings-page { max-width: 780px; }
h1 { margin: 5px 0 23px; font-size: clamp(28px, 4vw, 38px); letter-spacing: -.045em; }
.settings-panel { overflow: hidden; border: 1px solid var(--border); border-radius: var(--radius); background: var(--panel); }
.settings-row { display: flex; align-items: center; justify-content: space-between; gap: 14px; padding: 19px; }
.settings-row + .settings-row { border-top: 1px solid var(--border); }
.settings-row span { display: flex; align-items: center; gap: 12px; font-size: 14px; }
.settings-row svg { color: var(--accent); }
.settings-row input { width: 19px; height: 19px; accent-color: var(--accent); cursor: pointer; }
</style>

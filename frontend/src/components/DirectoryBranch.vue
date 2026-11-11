<script setup lang="ts">
import {computed, ref, watch} from 'vue'
import {ChevronRight, FolderClosed, FolderOpen, HardDrive, RefreshCw} from '@lucide/vue'
import {useI18n} from 'vue-i18n'
import {folders, joinPath, libraryRoute, loadDirectory} from '../library'

const props = defineProps<{path: string; name: string; current: string; depth: number}>()
const {t} = useI18n()
const expanded = ref(false)
const failed = ref(false)
const active = computed(() => props.path === props.current)
const children = computed(() => folders(props.path))

async function open() {
    expanded.value = true
    failed.value = false
    try {
        await loadDirectory(props.path)
    } catch {
        failed.value = true
    }
}

watch(() => props.current, (current) => {
    if (current === props.path || current.startsWith(props.path + '/')) void open()
}, {immediate: true})
</script>

<template>
    <li>
        <div class="tree-row" :class="{active}" :style="{'--depth': Math.min(depth, 5)}">
            <button class="tree-toggle" type="button" :aria-expanded="expanded"
                :aria-label="t(expanded ? 'collapse' : 'expand', {name})" @click="expanded ? expanded = false : open()">
                <ChevronRight :size="14" :class="{expanded}" />
            </button>
            <RouterLink class="tree-link" :to="libraryRoute(path)" :aria-current="active ? 'location' : undefined"
                :title="name" @click="open">
                <component :is="depth === 0 ? HardDrive : expanded ? FolderOpen : FolderClosed" :size="17" />
                <span>{{ name }}</span>
            </RouterLink>
        </div>
        <template v-if="expanded">
            <button v-if="failed" class="tree-retry" type="button" @click="open">
                <RefreshCw :size="13" />{{ t('retry') }}
            </button>
            <ul v-else-if="children.length" class="tree-list">
                <DirectoryBranch v-for="child in children" :key="child.name"
                    :path="joinPath(path, child.name)" :name="child.name" :current="current" :depth="depth + 1" />
            </ul>
        </template>
    </li>
</template>

<style scoped>
.tree-row {
    display: flex;
    align-items: center;
    padding-left: calc(var(--depth) * 13px);
    min-height: 37px;
    border-radius: 8px;
    color: var(--muted);
}

.tree-row:hover { background: var(--hover); color: var(--text); }
.tree-row.active { background: var(--accent-soft); color: var(--accent); }

.tree-toggle {
    display: grid;
    place-items: center;
    flex: 0 0 24px;
    width: 24px;
    height: 33px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: inherit;
}

.tree-toggle svg { transition: transform 160ms ease; }
.tree-toggle svg.expanded { transform: rotate(90deg); }

.tree-link {
    flex: 1;
    display: flex;
    align-items: center;
    min-width: 0;
    min-height: 37px;
    gap: 9px;
    padding-right: 8px;
    font-size: 13px;
}

.tree-link span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tree-link svg { flex-shrink: 0; }
.tree-retry {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 3px 0 3px 28px;
    color: var(--muted);
    border: 0;
    background: none;
    font-size: 12px;
}
</style>

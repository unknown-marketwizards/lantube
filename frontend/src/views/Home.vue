<script setup lang="ts">
import {computed, onMounted, ref, watch} from 'vue'
import {useRoute} from 'vue-router'
import {useI18n} from 'vue-i18n'
import {ArrowUp, ChevronRight, Film, House, LibraryBig, Menu, Settings, X} from '@lucide/vue'
import DirectoryBranch from '../components/DirectoryBranch.vue'
import {directoryPath, folders, joinPath, libraryRoute, loadDirectory, parentPath} from '../library'

const route = useRoute()
const {t} = useI18n()
const drawer = ref(false)
const current = computed(() => directoryPath(route.query.dir))
const sources = computed(() => folders('/'))
const breadcrumbs = computed(() => {
    let path = '/'
    return current.value.split('/').filter(Boolean).map((name) => {
        path = joinPath(path, name)
        return {name, path}
    })
})

watch(() => route.fullPath, () => {
    drawer.value = false
})
onMounted(() => void loadDirectory('/').catch(() => {}))
</script>

<template>
    <div class="app-shell" @keydown.esc="drawer = false">
        <button v-if="drawer" type="button" class="drawer-shade" :aria-label="t('closeNavigation')" @click="drawer = false"></button>
        <aside class="sidebar" :class="{'drawer-open': drawer}" id="library-navigation">
            <RouterLink class="brand" :to="libraryRoute('/')" @click="drawer = false">
                <span class="brand-mark"><Film :size="19" :stroke-width="2.4" /></span>
                <span>Lan<span class="brand-accent">Tube</span></span>
            </RouterLink>

            <nav class="nav" :aria-label="t('library')">
                <RouterLink class="nav-link" :class="{'nav-active': route.name === 'library' || route.name === 'player'}"
                    :to="libraryRoute('/')" @click="drawer = false">
                    <LibraryBig :size="18" /><span>{{ t('library') }}</span>
                </RouterLink>
                <RouterLink class="nav-link" :class="{'nav-active': route.name === 'settings'}"
                    :to="{name: 'settings'}" @click="drawer = false">
                    <Settings :size="18" /><span>{{ t('settings') }}</span>
                </RouterLink>
            </nav>

            <div class="source-nav">
                <div class="source-heading">{{ t('sources') }}</div>
                <ul class="tree-list">
                    <DirectoryBranch v-for="source in sources" :key="source.name"
                        :name="source.name" :path="joinPath('/', source.name)" :depth="0" :current="current" />
                </ul>
            </div>
            <div class="sidebar-footer"><span class="local-dot"></span>{{ t('local') }}</div>
        </aside>

        <div class="workspace">
            <header class="topbar">
                <button class="button icon quiet drawer-toggle" type="button" :aria-label="t('openNavigation')"
                    aria-controls="library-navigation" :aria-expanded="drawer" @click="drawer = !drawer">
                    <component :is="drawer ? X : Menu" :size="20" />
                </button>

                <nav v-if="route.name !== 'settings'" class="crumbs" :aria-label="t('library')">
                    <RouterLink class="crumb root-crumb" :to="libraryRoute('/')" :title="t('library')">
                        <House :size="16" /><span>{{ t('library') }}</span>
                    </RouterLink>
                    <template v-for="crumb in breadcrumbs" :key="crumb.path">
                        <ChevronRight class="crumb-divider" :size="13" aria-hidden="true" />
                        <RouterLink :class="['crumb', {current: crumb.path === current}]" :to="libraryRoute(crumb.path)"
                            :title="crumb.name" :aria-current="crumb.path === current && route.name === 'library' ? 'location' : undefined">
                            {{ crumb.name }}
                        </RouterLink>
                    </template>
                </nav>
                <span v-else class="settings-crumb"><Settings :size="17" />{{ t('settings') }}</span>

                <RouterLink v-if="route.name === 'library' && current !== '/'" class="button quiet top-action"
                    :to="libraryRoute(parentPath(current))" :aria-label="t('parent')" :title="t('parent')">
                    <ArrowUp :size="16" /><span>{{ t('parent') }}</span>
                </RouterLink>
            </header>
            <main class="content" id="main-content">
                <RouterView />
            </main>
        </div>
    </div>
</template>

<style scoped>
.app-shell { min-height: 100vh; }
.sidebar {
    position: fixed;
    inset: 0 auto 0 0;
    z-index: 5;
    display: flex;
    flex-direction: column;
    width: var(--sidebar-width);
    height: 100dvh;
    padding: 26px 15px 18px;
    border-right: 1px solid var(--border);
    background: var(--rail);
}

.brand {
    display: flex;
    align-items: center;
    gap: 11px;
    align-self: flex-start;
    margin: 0 10px 30px;
    font-size: 23px;
    font-weight: 740;
    letter-spacing: -.9px;
}
.brand-mark {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: 10px;
    background: var(--accent);
    box-shadow: 0 4px 18px #ff796b26;
    color: #251416;
}
.brand-accent { color: var(--accent); }
.nav { display: grid; gap: 4px; }
.nav-link {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 43px;
    padding: 0 13px;
    border-radius: 9px;
    color: var(--muted);
    font-size: 14px;
    font-weight: 550;
}
.nav-link:hover { background: var(--hover); color: var(--text); }
.nav-link.nav-active { color: var(--accent); background: var(--accent-soft); }

.source-nav {
    flex: 1;
    overflow-y: auto;
    overscroll-behavior: contain;
    margin: 24px -6px 10px;
    padding: 18px 6px 8px;
    border-top: 1px solid var(--border);
    scrollbar-width: thin;
}
.source-heading {
    margin: 0 12px 11px;
    color: var(--subtle);
    font-size: 11px;
    font-weight: 650;
    letter-spacing: .075em;
    text-transform: uppercase;
}
.sidebar-footer {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 15px 11px 0;
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: 12px;
}
.local-dot { width: 7px; height: 7px; border-radius: 50%; background: #78c7a3; }

.workspace { margin-left: var(--sidebar-width); }
.topbar {
    position: sticky;
    top: 0;
    z-index: 3;
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 67px;
    padding: 10px clamp(20px, 3.1vw, 45px);
    border-bottom: 1px solid var(--border);
    background: #101217ee;
    backdrop-filter: blur(14px);
}
.crumbs, .settings-crumb {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
    flex: 1;
    overflow-x: auto;
    scrollbar-width: none;
    color: var(--muted);
    font-size: 13px;
    font-weight: 530;
}
.crumb {
    max-width: 28ch;
    min-height: 32px;
    flex: 0 0 auto;
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 32px;
}
.crumb.root-crumb { display: flex; align-items: center; gap: 8px; }
.crumb.current, .crumb:hover { color: var(--text); }
.crumb-divider { color: var(--subtle); }
.top-action { margin-left: auto; min-height: 35px; }
.content {
    width: 100%;
    max-width: 1660px;
    margin: 0 auto;
    padding: 28px clamp(20px, 3.1vw, 45px) 54px;
}
.drawer-toggle, .drawer-shade { display: none; }

@media (max-width: 890px) {
    .workspace { margin-left: 0; }
    .sidebar { transform: translateX(-100%); visibility: hidden; transition: transform 200ms ease, visibility 200ms; }
    .sidebar.drawer-open { transform: translateX(0); visibility: visible; }
    .drawer-toggle { display: inline-flex; }
    .drawer-shade {
        display: block;
        position: fixed;
        inset: 0;
        z-index: 4;
        border: 0;
        background: #080a0dcc;
    }
}
@media (max-width: 540px) {
    .topbar { min-height: 57px; padding: 7px 14px; gap: 8px; }
    .content { padding: 15px 14px 36px; }
    .top-action { width: 36px; padding: 0; }
    .top-action span { display: none; }
    .crumb { max-width: 18ch; }
    .crumbs { gap: 6px; }
    .root-crumb span { display: none; }
}
</style>

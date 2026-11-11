<script setup lang="ts">
import {computed, onBeforeUnmount, onMounted, ref, watch} from 'vue'
import {useRoute, useRouter} from 'vue-router'
import {useI18n} from 'vue-i18n'
import {
    ArrowRight, CircleAlert, Clapperboard, File, Files, FolderClosed, FolderOpen, FolderPlus, Grid2X2,
    HardDrive, List, LoaderCircle, Play, Search, Upload, X,
} from '@lucide/vue'
import {createDirectory, uploadFile} from '../api'
import MediaCover from '../components/MediaCover.vue'
import {
    canWrite, directoryPath, extension, folders, formatSize, isAudio, joinPath, libraryRoute, loadDirectory,
    mediaUrl, orderedFiles, pathName, playerRoute, title, useDirectory,
} from '../library'
import {preferences} from '../preferences'

const {t} = useI18n()
const route = useRoute()
const router = useRouter()
const path = computed(() => directoryPath(route.query.dir))
const atRoot = computed(() => path.value === '/')
const showOther = computed(() => !atRoot.value && route.query.other === '1')
const heading = computed(() => atRoot.value ? t('library') : pathName(path.value))
const {items, pending, error, reload} = useDirectory(path, showOther)
const canManageFolder = computed(() => canWrite.value && !atRoot.value && !pending.value && !error.value)

const query = ref('')
const searchInput = ref<HTMLInputElement>()
const searchTerm = computed(() => query.value.trim().toLocaleLowerCase())
const allFolders = computed(() => folders(path.value))
const files = computed(() => orderedFiles(items.value))
const otherFiles = computed(() => showOther.value ? orderedFiles(items.value, 'other') : [])
const visibleFolders = computed(() => allFolders.value.filter((entry) => matches(entry.name)))
const visibleFiles = computed(() => files.value.filter((entry) => matches(entry.name)))
const visibleOtherFiles = computed(() => otherFiles.value.filter((entry) => matches(entry.name)))
const firstFolder = computed(() => allFolders.value[0])
const firstFile = computed(() => files.value[0])

function matches(name: string) {
    return name.toLocaleLowerCase().includes(searchTerm.value)
}

watch(path, () => {
    query.value = ''
    folderDialog.value?.close()
})

function focusSearch(event: KeyboardEvent) {
    if (event.defaultPrevented || event.key !== '/' || event.metaKey || event.ctrlKey || event.altKey) return
    if ((event.target as HTMLElement | null)?.closest('input, textarea, select, [contenteditable], dialog')) return
    event.preventDefault()
    searchInput.value?.focus()
}

onMounted(() => window.addEventListener('keydown', focusSearch))
onBeforeUnmount(() => window.removeEventListener('keydown', focusSearch))

const folderDialog = ref<HTMLDialogElement>()
const folderName = ref('')
const folderError = ref('')
const creating = ref(false)

function openFolderDialog() {
    folderName.value = ''
    folderError.value = ''
    folderDialog.value?.showModal()
}

async function makeFolder() {
    if (!canManageFolder.value || creating.value) return
    const name = folderName.value.trim()
    if (!name || name === '.' || name === '..' || name.includes('/') || name.includes('\\')) {
        folderError.value = t('invalidName')
        return
    }
    const destination = path.value
    creating.value = true
    folderError.value = ''
    try {
        await createDirectory(joinPath(destination, name))
        await loadDirectory(destination, {refresh: true})
        folderDialog.value?.close()
    } catch (cause) {
        folderError.value = (cause as Error).message
    } finally {
        creating.value = false
    }
}

const fileInput = ref<HTMLInputElement>()
const upload = ref<{name: string; index: number; total: number; percent: number}>()
const uploadResult = ref<{count: number; errors: string[]}>()
const dragDepth = ref(0)
const canUpload = computed(() => canManageFolder.value && !upload.value)

async function sendFiles(selected: File[]) {
    if (!selected.length || !canUpload.value) return
    const destination = path.value
    uploadResult.value = undefined
    const errors: string[] = []
    let count = 0
    for (const [index, file] of selected.entries()) {
        const current = {name: file.name, index: index + 1, total: selected.length, percent: 0}
        upload.value = current
        try {
            await uploadFile(destination, file, (percent) => {
                if (upload.value) upload.value.percent = percent
            })
            count++
        } catch (cause) {
            errors.push(file.name + ' · ' + (cause as Error).message)
        }
    }
    try {
        await loadDirectory(destination, {refresh: true})
    } catch (cause) {
        if (destination === path.value) error.value = (cause as Error).message
    } finally {
        upload.value = undefined
        uploadResult.value = {count, errors}
    }
}

function chooseFiles(event: Event) {
    const input = event.target as HTMLInputElement
    const selected = Array.from(input.files ?? [])
    input.value = ''
    void sendFiles(selected)
}

function isFileDrag(event: DragEvent) {
    return event.dataTransfer?.types.includes('Files')
}

function dragEnter(event: DragEvent) {
    if (!isFileDrag(event)) return
    event.preventDefault()
    dragDepth.value++
}

function dragOver(event: DragEvent) {
    if (!isFileDrag(event) || !event.dataTransfer) return
    event.preventDefault()
    event.dataTransfer.dropEffect = canUpload.value ? 'copy' : 'none'
}

function dropFiles(event: DragEvent) {
    if (!isFileDrag(event)) return
    event.preventDefault()
    dragDepth.value = 0
    void sendFiles(Array.from(event.dataTransfer?.files ?? []))
}
</script>

<template>
    <div class="library-page" @dragenter="dragEnter" @dragover="dragOver"
        @dragleave="dragDepth = Math.max(0, dragDepth - 1)" @drop="dropFiles">
        <section class="masthead" aria-labelledby="library-title">
            <div class="masthead-content">
                <p class="masthead-label"><component :is="atRoot ? Clapperboard : FolderOpen" :size="16" />
                    {{ atRoot ? t('local') : t('library') }}
                </p>
                <h1 id="library-title">{{ heading }}</h1>
                <p v-if="!pending && !error" class="masthead-counts">
                    <span>{{ t('folderCount', allFolders.length) }}</span>
                    <template v-if="!atRoot"><span class="count-divider"></span><span>{{ t('mediaCount', files.length) }}</span></template>
                </p>
                <div v-if="firstFile || firstFolder" class="masthead-actions">
                    <RouterLink v-if="firstFile" class="button primary" :to="playerRoute(path, firstFile.name)">
                        <Play :size="16" fill="currentColor" />{{ t('playAll') }}
                    </RouterLink>
                    <RouterLink v-else-if="firstFolder" class="button primary" :to="libraryRoute(joinPath(path, firstFolder.name))">
                        {{ t('openFolder', {name: firstFolder.name}) }}<ArrowRight :size="16" />
                    </RouterLink>
                </div>
            </div>
        </section>

        <div class="library-toolbar">
            <label class="search">
                <Search :size="17" aria-hidden="true" />
                <input ref="searchInput" v-model="query" type="search" :aria-label="t('search')"
                    :placeholder="t('search')" @keydown.esc="query = ''">
                <button v-if="query" type="button" :aria-label="t('clearSearch')" @click="query = ''; searchInput?.focus()">
                    <X :size="15" />
                </button>
                <kbd v-else aria-hidden="true">/</kbd>
            </label>
            <div v-if="!atRoot" class="toolbar-actions">
                <button class="button" type="button" :aria-pressed="showOther" :disabled="pending"
                    @click="router.replace(libraryRoute(path, !showOther))">
                    <Files /><span>{{ t('showOtherFiles') }}</span>
                </button>
                <template v-if="canWrite">
                    <button class="button" type="button" :disabled="!canManageFolder" @click="openFolderDialog">
                        <FolderPlus /><span>{{ t('newFolder') }}</span>
                    </button>
                    <button class="button primary" type="button" :disabled="!canUpload" @click="fileInput?.click()">
                        <Upload /><span>{{ t('upload') }}</span>
                    </button>
                    <input ref="fileInput" type="file" multiple hidden @change="chooseFiles">
                </template>
            </div>
        </div>

        <div v-if="upload" class="upload-notice" role="status">
            <Upload :size="19" />
            <div class="upload-copy">
                <span>{{ t('uploading', upload) }}</span>
                <progress :value="upload.percent" max="100"></progress>
            </div>
            <span>{{ upload.percent }}%</span>
        </div>
        <div v-else-if="uploadResult" class="upload-notice" role="status">
            <component :is="uploadResult.errors.length ? CircleAlert : Upload" :size="19" />
            <div class="upload-copy">
                <strong v-if="uploadResult.count">{{ t('uploadDone', uploadResult.count) }}</strong>
                <details v-if="uploadResult.errors.length">
                    <summary>{{ t('uploadFailed') }}</summary>
                    <p v-for="(message, index) in uploadResult.errors" :key="index">{{ message }}</p>
                </details>
            </div>
            <button class="button icon quiet" type="button" :aria-label="t('dismiss')" @click="uploadResult = undefined"><X /></button>
        </div>

        <div v-if="error" class="status-panel" role="alert">
            <CircleAlert :size="28" /><strong>{{ t('loadFailed') }}</strong><p>{{ error }}</p>
            <button class="button" type="button" @click="reload">{{ t('retry') }}</button>
        </div>
        <div v-else-if="pending && !items.length" class="status-panel" role="status">
            <LoaderCircle class="spinner" :size="26" /><strong>{{ t('loading') }}</strong>
        </div>
        <div v-else-if="!allFolders.length && !files.length && !otherFiles.length" class="status-panel">
            <component :is="atRoot ? HardDrive : FolderOpen" :size="32" />
            <strong>{{ t(atRoot ? 'noSources' : 'emptyFolder') }}</strong>
            <p v-if="atRoot || canWrite">{{ t(atRoot ? 'noSourcesHelp' : 'emptyFolderHelp') }}</p>
        </div>
        <div v-else-if="!visibleFolders.length && !visibleFiles.length && !visibleOtherFiles.length" class="status-panel">
            <Search :size="28" /><strong>{{ t('noResults') }}</strong>
            <button class="button" type="button" @click="query = ''">{{ t('clearSearch') }}</button>
        </div>

        <template v-else>
            <section v-if="visibleFolders.length" class="library-section" aria-labelledby="folder-heading">
                <header class="section-heading">
                    <h2 id="folder-heading">{{ t(atRoot ? 'sources' : 'folders') }}</h2>
                    <span class="section-count">{{ visibleFolders.length }}</span>
                </header>
                <div class="folder-grid">
                    <RouterLink v-for="folder in visibleFolders" :key="folder.name" class="folder-card"
                        :to="libraryRoute(joinPath(path, folder.name))" :title="folder.name">
                        <span class="folder-icon"><component :is="atRoot ? HardDrive : FolderClosed" :size="22" /></span>
                        <span class="folder-copy"><strong>{{ folder.name }}</strong></span>
                        <ArrowRight class="folder-arrow" :size="16" />
                    </RouterLink>
                </div>
            </section>

            <section v-if="visibleFiles.length" class="library-section" aria-labelledby="media-heading">
                <header class="section-heading media-heading">
                    <h2 id="media-heading">{{ t('media') }}</h2>
                    <span class="section-count">{{ visibleFiles.length }}</span>
                    <div class="view-options">
                        <select v-model="preferences.sort" class="select" :aria-label="t('sort')">
                            <option value="name">{{ t('sortName') }}</option>
                            <option value="recent">{{ t('sortRecent') }}</option>
                            <option value="size">{{ t('sortSize') }}</option>
                        </select>
                        <div class="view-toggle" :aria-label="t('view')" role="group">
                            <button type="button" :aria-label="t('grid')" :title="t('grid')"
                                :aria-pressed="preferences.view === 'grid'" @click="preferences.view = 'grid'"><Grid2X2 :size="16" /></button>
                            <button type="button" :aria-label="t('list')" :title="t('list')"
                                :aria-pressed="preferences.view === 'list'" @click="preferences.view = 'list'"><List :size="18" /></button>
                        </div>
                    </div>
                </header>

                <div class="media-grid" :class="{'list-view': preferences.view === 'list'}">
                    <RouterLink v-for="file in visibleFiles" :key="file.name" class="media-card"
                        :to="playerRoute(path, file.name)" :title="file.name">
                        <div class="card-cover">
                            <MediaCover :src="mediaUrl(path, file)" :audio="isAudio(file.name)" />
                            <span class="card-play"><Play :size="20" fill="currentColor" /></span>
                        </div>
                        <div class="card-copy">
                            <strong>{{ title(file.name) }}</strong>
                            <span class="card-meta"><span>{{ extension(file.name).toUpperCase() }}</span>
                                <template v-if="file.size !== undefined"><span>·</span><span>{{ formatSize(file.size) }}</span></template>
                            </span>
                        </div>
                        <ArrowRight class="row-arrow" :size="18" />
                    </RouterLink>
                </div>
            </section>
            <section v-if="visibleOtherFiles.length" class="library-section" aria-labelledby="other-files-heading">
                <header class="section-heading">
                    <h2 id="other-files-heading">{{ t('otherFiles') }}</h2>
                    <span class="section-count">{{ visibleOtherFiles.length }}</span>
                </header>
                <div class="folder-grid">
                    <RouterLink v-for="file in visibleOtherFiles" :key="file.name" class="folder-card"
                        :to="playerRoute(path, file.name)" :title="file.name">
                        <span class="folder-icon"><File :size="22" /></span>
                        <span class="folder-copy"><strong>{{ file.name }}</strong></span>
                        <ArrowRight class="folder-arrow" :size="18" />
                    </RouterLink>
                </div>
            </section>
        </template>

        <dialog ref="folderDialog" class="folder-dialog" aria-labelledby="new-folder-title">
            <form @submit.prevent="makeFolder">
                <h2 id="new-folder-title">{{ t('newFolder') }}</h2>
                <label>{{ t('folderName') }}<input v-model="folderName" required autofocus :disabled="creating"></label>
                <p v-if="folderError" class="form-error" role="alert">{{ folderError }}</p>
                <div class="dialog-actions">
                    <button class="button" type="button" :disabled="creating" @click="folderDialog?.close()">{{ t('cancel') }}</button>
                    <button class="button primary" type="submit" :disabled="creating">{{ t('create') }}</button>
                </div>
            </form>
        </dialog>

        <div v-if="dragDepth && canUpload" class="drop-overlay" aria-hidden="true">
            <Upload :size="44" /><strong>{{ t('dropFiles') }}</strong>
        </div>
    </div>
</template>

<style scoped>
.library-page { display: grid; gap: 25px; }

.masthead {
    position: relative;
    display: flex;
    align-items: end;
    min-height: clamp(235px, 23vw, 326px);
    overflow: hidden;
    isolation: isolate;
    padding: clamp(24px, 3.4vw, 48px);
    border: 1px solid #3b485b;
    border-radius: 17px;
    background: linear-gradient(90deg, #10151fe8 0%, #10151f8a 46%, #10151f0d 82%),
        linear-gradient(0deg, #101217b5, transparent 55%),
        #233342 url('../assets/library-cover.webp') center right / cover no-repeat;
}
.masthead-content { min-width: 0; width: min(100%, 740px); display: grid; gap: 13px; }
.masthead-label {
    display: flex;
    align-items: center;
    gap: 8px;
    color: #f5a298;
    font-size: 12px;
    font-weight: 690;
    letter-spacing: .055em;
    text-transform: uppercase;
}
.masthead h1 {
    color: #fff;
    font-size: clamp(32px, 4.5vw, 58px);
    font-weight: 730;
    letter-spacing: -.045em;
    line-height: 1.12;
    overflow-wrap: anywhere;
}
.masthead-counts { display: flex; align-items: center; gap: 10px; color: #c5cdd9; font-size: 14px; }
.count-divider { width: 4px; height: 4px; border-radius: 50%; background: currentColor; }
.masthead-actions { margin-top: 8px; }
.masthead-actions .button { padding-inline: 15px; min-height: 43px; }

.library-toolbar { display: flex; justify-content: space-between; gap: 12px; align-items: center; }
.search {
    display: flex;
    align-items: center;
    gap: 10px;
    width: min(100%, 354px);
    min-width: 0;
    min-height: 42px;
    padding: 0 11px;
    color: var(--subtle);
    border: 1px solid var(--border);
    border-radius: 9px;
    background: var(--panel);
}
.search:focus-within { border-color: var(--accent); }
.search input { flex: 1; width: 100%; min-width: 0; padding: 9px 0; border: 0; outline: none; background: none; color: var(--text); font-size: 13px; }
.search input::-webkit-search-cancel-button { display: none; }
.search input::placeholder { color: var(--muted); }
.search button { display: grid; place-items: center; padding: 3px; border: 0; border-radius: 4px; background: none; color: var(--muted); }
.search button:hover { color: var(--text); background: var(--hover); }
.search kbd { padding: 2px 6px; border: 1px solid var(--border); border-radius: 4px; font-size: 11px; line-height: 1.2; }
.toolbar-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 8px; }
.toolbar-actions button[aria-pressed=true] { background: var(--accent-soft); color: var(--accent); }

.library-section { min-width: 0; }
.section-heading { display: flex; align-items: center; gap: 11px; margin-bottom: 13px; min-height: 32px; }
.section-heading h2 { font-size: 21px; font-weight: 670; letter-spacing: -.025em; }
.section-count { display: inline-grid; place-items: center; min-width: 25px; padding: 2px 7px; border-radius: 20px; background: var(--raised); color: var(--muted); font-size: 12px; font-variant-numeric: tabular-nums; }
.folder-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 222px), 1fr)); gap: 12px; }
.folder-card {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    min-height: 74px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 11px;
    background: var(--panel);
    transition: border-color 160ms ease, background 160ms ease;
}
.folder-card:hover { background: var(--hover); border-color: #4b4c59; }
.folder-icon { display: grid; place-items: center; flex-shrink: 0; width: 42px; height: 42px; border-radius: 9px; background: var(--accent-soft); color: var(--accent); }
.folder-copy { flex: 1; display: grid; min-width: 0; }
.folder-copy strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 14px; font-weight: 620; }
.folder-arrow { margin-left: auto; color: var(--subtle); }
.folder-card:hover .folder-arrow { color: var(--accent); }

.view-options { display: flex; align-items: center; gap: 9px; margin-left: auto; }
.view-options .select { min-height: 36px; }
.view-toggle { display: flex; padding: 3px; border: 1px solid var(--border); border-radius: 9px; background: var(--panel); }
.view-toggle button { display: grid; place-items: center; width: 31px; height: 28px; border: 0; border-radius: 5px; color: var(--muted); background: transparent; }
.view-toggle button[aria-pressed=true] { background: var(--raised); color: var(--accent); }

.media-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 205px), 1fr)); gap: 16px; }
.media-card { display: block; min-width: 0; overflow: hidden; border: 1px solid var(--border); border-radius: 11px; background: var(--panel); transition: border-color 160ms ease, background 160ms ease; }
.media-card:hover { border-color: #555865; background: var(--hover); }
.card-cover { position: relative; overflow: hidden; }
.card-play { position: absolute; inset: 50% auto auto 50%; display: grid; place-items: center; width: 44px; height: 44px; transform: translate(-50%, -44%); border: 1px solid #ffffff42; border-radius: 50%; background: #13151bdb; color: #fff; opacity: 0; transition: opacity 160ms ease, transform 160ms ease; }
.card-play svg { margin-left: 2px; }
.media-card:is(:hover, :focus-visible) .card-play { opacity: 1; transform: translate(-50%, -50%); }
.card-copy { display: grid; gap: 7px; min-width: 0; padding: 12px 13px 13px; }
.card-copy strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 14px; font-weight: 620; }
.card-meta { display: flex; align-items: center; gap: 7px; color: var(--muted); font-size: 12px; }
.card-meta span:first-child { font-size: 10px; font-weight: 690; letter-spacing: .055em; }
.row-arrow { display: none; }
.list-view { grid-template-columns: 1fr; gap: 7px; }
.list-view .media-card { display: flex; gap: 13px; align-items: center; padding: 8px 15px 8px 8px; }
.list-view .card-cover { flex: 0 0 105px; border-radius: 6px; }
.list-view .card-copy { flex: 1; padding: 0; }
.list-view .row-arrow { display: block; color: var(--muted); }

.upload-notice { display: flex; align-items: center; gap: 12px; padding: 12px 14px; border: 1px solid var(--border); border-radius: 9px; background: var(--panel); color: var(--text); font-size: 13px; }
.upload-notice > svg { color: var(--accent); }
.upload-copy { display: grid; gap: 6px; flex: 1; min-width: 0; overflow-wrap: anywhere; }
.upload-copy p { padding-block: 3px; color: var(--muted); }
.upload-copy progress { width: 100%; height: 7px; accent-color: var(--accent); }
.upload-notice .button { min-width: 31px; min-height: 31px; }
.drop-overlay { position: fixed; inset: 75px 22px 22px calc(var(--sidebar-width) + 22px); z-index: 4; display: grid; place-content: center; justify-items: center; gap: 14px; border: 2px dashed var(--accent); border-radius: 16px; background: #181a23ed; color: var(--accent); pointer-events: none; }
.drop-overlay strong { font-size: clamp(19px, 3vw, 27px); }

.folder-dialog { width: min(420px, calc(100% - 32px)); padding: 23px; border: 1px solid var(--border); border-radius: 15px; background: var(--panel); color: var(--text); }
.folder-dialog::backdrop { background: #07090cc2; }
.folder-dialog h2 { font-size: 20px; }
.folder-dialog label { display: grid; gap: 8px; margin-top: 18px; color: var(--muted); font-size: 13px; }
.folder-dialog input { min-width: 0; padding: 10px; border: 1px solid var(--border); border-radius: 8px; background: var(--bg); color: var(--text); }
.dialog-actions { display: flex; justify-content: end; gap: 8px; margin-top: 20px; }
.form-error { margin-top: 8px; color: var(--accent); font-size: 13px; }
.spinner { animation: spin 1s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }

@media (max-width: 890px) { .drop-overlay { left: 22px; } }
@media (max-width: 590px) {
    .library-page { gap: 19px; }
    .masthead { min-height: 225px; padding: 21px; border-radius: 12px; }
    .masthead h1 { font-size: clamp(29px, 7vw, 40px); }
    .masthead-counts { font-size: 12px; }
    .library-toolbar { flex-wrap: wrap; }
    .search { width: 100%; }
    .toolbar-actions { width: 100%; justify-content: flex-end; }
    .folder-grid, .media-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 9px; }
    .folder-card { min-height: 66px; gap: 8px; padding: 8px; }
    .folder-icon { width: 34px; height: 34px; }
    .folder-icon svg { width: 18px; }
    .folder-arrow { display: none; }
    .folder-copy strong, .card-copy strong { font-size: 13px; }
    .media-heading h2 { font-size: 18px; }
    .view-options { gap: 5px; }
    .view-options .select { max-width: 114px; padding-left: 7px; padding-right: 4px; }
    .list-view { grid-template-columns: 1fr; }
    .card-copy { padding: 9px; }
}
</style>

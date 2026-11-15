<script setup lang="ts">
import {computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch} from 'vue'
import {useRoute, useRouter} from 'vue-router'
import {useI18n} from 'vue-i18n'
import type Hls from 'hls.js'
import {
    ArrowLeft, CircleAlert, Clock3, Download, Film, ListVideo, LoaderCircle, Maximize, Minimize,
    Crosshair, ListTree, Music2, Pause, PictureInPicture2, Play, Repeat2, Scissors, SkipBack, SkipForward, Volume2, VolumeX, X,
} from '@lucide/vue'
import type {Chapter, ExplorerItem} from '../api'
import {chapterAt, chapterTime, maxChapters, maxChapterTitle, minChapterMs, parseChapterTime, useChapters} from '../chapters'
import ChapterList from '../components/ChapterList.vue'
import MediaCover from '../components/MediaCover.vue'
import {
    canWrite, conversionUrl, directoryPath, extension, findUnlistedFile, formatSize, isAudio, joinPath, libraryRoute, mediaUrl,
    orderedFiles, playerRoute, title, useDirectory,
} from '../library'
import {unloadMedia, useMediaSource} from '../mediaSource'
import {formatTime} from '../thumbnails'

const {t} = useI18n()
const route = useRoute()
const router = useRouter()
const directory = computed(() => directoryPath(route.query.dir))
const fileName = computed(() => typeof route.query.file === 'string' ? route.query.file : undefined)
const {items, pending, error, reload} = useDirectory(directory)
const files = computed(() => orderedFiles(items.value))
const unlisted = shallowRef<ExplorerItem>()
const findingFile = ref(false)
const findError = ref('')
const lookupName = computed(() => pending.value || error.value
    || files.value.some((item) => item.name === fileName.value) ? undefined : fileName.value)
const playlist = computed(() => unlisted.value ? [unlisted.value] : files.value)
const index = computed(() => fileName.value !== undefined
    ? playlist.value.findIndex((item) => item.name === fileName.value)
    : 0)
const current = computed(() => playlist.value[index.value])
const backToLibrary = computed(() => libraryRoute(directory.value, current.value?.type === 'other'))
const fileSource = computed(() => current.value ? mediaUrl(directory.value, current.value) : '')
const audio = computed(() => !!current.value && isAudio(current.value.name))
const audioOnly = ref(false)
const directPicture = ref<boolean>()

const stage = ref<HTMLElement>()
const video = ref<HTMLVideoElement>()
const queue = ref<HTMLElement>()
const playing = ref(false)
type CompatibilityFailure = 'transcodeUnavailable' | 'transcodeFailed' | 'transcodeUnsupported' | 'sourceChanged'
const failure = ref<CompatibilityFailure>()
const directFailed = ref(false)
const compatible = ref(false)
const audioLayout = computed(() => !compatible.value && !directFailed.value && (directPicture.value === undefined ? audio.value : !directPicture.value))
const playbackBlocked = computed(() => compatible.value ? !!failure.value : directFailed.value)
const compatibilityProbe = shallowRef<AbortController>()
const waitingForPicture = ref(false)
const pictureAvailable = computed(() => compatible.value ? !waitingForPicture.value && !failure.value : directPicture.value === true)
const preparing = computed(() => waitingForPicture.value || (directFailed.value && !!compatibilityProbe.value))
let hls: Hls | undefined
let playRequested = false
let attemptedCompatibility = false
let playbackGeneration = 0
const duration = ref(0)
const time = ref(0)
const volume = ref(1)
const muted = ref(false)
const speed = ref(1)
const fullscreen = ref(false)
const playMode = ref<'automatic' | 'manual'>('automatic')
const sleepMinutes = ref(0)
let timer: number | undefined

const {src, revision: mediaRevision, loading: mediaLoading, failed: mediaLoadFailed, reload: reloadMedia} =
    useMediaSource(fileSource, unloadPlayback)
const chapterPath = computed(() => current.value ? joinPath(directory.value, current.value.name) : '')
const {
    chapters, loading: chapterLoading, saving: chapterSaving, issue: chapterIssue,
    ready: chaptersReady, reload: reloadChapters, save: commitChapters,
} = useChapters(chapterPath, mediaRevision, reloadMedia)
const canEditChapters = computed(() => canWrite.value && chaptersReady.value)
const canSeek = computed(() => !preparing.value && !playbackBlocked.value && duration.value > 0)
const canSplit = computed(() => canEditChapters.value && editStart.value === undefined && !chapterSaving.value
    && chapterIssue.value !== 'chapterConflict' && canSeek.value)
const activeChapter = computed(() => chapterAt(chapters.value, time.value))
const chapterTitle = (chapter: Chapter, index: number) => chapter.title || t('chapterNumber', {number: index + 1})
const currentChapter = computed(() => chapters.value[activeChapter.value])
const railTab = ref<'queue' | 'chapters'>('queue')
watch(chapterPath, () => { railTab.value = 'queue' }, {flush: 'sync'})
const chapterPicker = ref<HTMLDialogElement>()
const pickerOpen = ref(false)
const chapterEditor = ref<HTMLFormElement>()
const editStart = ref<number>()
const editTitle = ref('')
const editTime = ref('')
const editIssue = ref<'chapterTimeInvalid' | 'chapterTitleInvalid'>()
const editIndex = computed(() => chapters.value.findIndex(chapter => chapter.startMs === editStart.value))
const editDraft = computed(() => editStart.value === 0 ? 0 : parseChapterTime(editTime.value))
const otherChapters = computed(() => chapters.value.filter(chapter => chapter.startMs !== editStart.value))
const playheadMs = computed(() => Math.round(time.value * 1000))
const canPreviewCut = computed(() => canSeek.value && (editDraft.value === editStart.value || validCut(editDraft.value, otherChapters.value))
    && editDraft.value / 1000 < duration.value)
const canUsePlayhead = computed(() => canSeek.value && (playheadMs.value === editStart.value || validCut(playheadMs.value, otherChapters.value))
    && playheadMs.value !== editDraft.value)
const editSteps = [-1000, -100, 100, 1000]
let editTrigger: HTMLElement | undefined
const chapterNotice = ref<{key: 'chapterAdded' | 'chapterTooClose' | 'chapterLimit'; time?: string}>()
const chapterSegments = computed(() => {
    const end = duration.value * 1000
    return chapters.value.flatMap((chapter, index) => {
        if (chapter.startMs >= end) return []
        const length = Math.min(chapters.value[index + 1]?.startMs ?? end, end) - chapter.startMs
        const filled = Math.max(0, Math.min(1, (time.value * 1000 - chapter.startMs) / length))
        return [{start: chapter.startMs, width: length / end * 100 + '%', fill: filled * 100 + '%'}]
    })
})

function seekToCut(startMs: number) {
    if (!video.value || startMs / 1000 >= duration.value) return
    video.value.currentTime = startMs / 1000
    mediaTime()
}

function selectChapter(chapter: Chapter) {
    seekToCut(chapter.startMs)
    chapterPicker.value?.close()
}

function openChapterPicker() {
    chapterPicker.value?.showModal()
    pickerOpen.value = true
}

function validCut(startMs: number, chaptersToCheck: Chapter[]): boolean {
    return Number.isSafeInteger(startMs)
        && startMs >= minChapterMs
        && startMs <= duration.value * 1000 - minChapterMs
        && chaptersToCheck.every(chapter => Math.abs(chapter.startMs - startMs) >= minChapterMs)
}

async function splitChapter() {
    if (!canSplit.value || !video.value) return
    const at = Math.round(video.value.currentTime * 1000)
    const source = src.value
    const next = chapters.value.length ? chapters.value : [{startMs: 0, title: ''}]
    railTab.value = 'chapters'
    chapterNotice.value = undefined
    if (next.length >= maxChapters) {
        chapterNotice.value = {key: 'chapterLimit'}
        return
    }
    if (!validCut(at, next)) {
        chapterNotice.value = {key: 'chapterTooClose'}
        return
    }
    const newChapters = [...next, {startMs: at, title: ''}].sort((a, b) => a.startMs - b.startMs)
    if (await commitChapters(newChapters) && src.value === source) {
        chapterNotice.value = {key: 'chapterAdded', time: chapterTime(at)}
    }
}

async function openChapterEditor(chapter: Chapter) {
    const fromPicker = chapterPicker.value?.open
    editTrigger = fromPicker
        ? undefined
        : document.activeElement instanceof HTMLElement ? document.activeElement : undefined
    chapterPicker.value?.close()
    pause()
    editStart.value = chapter.startMs
    editTitle.value = chapter.title
    editTime.value = chapterTime(chapter.startMs)
    editIssue.value = undefined
    chapterNotice.value = undefined
    await nextTick()
    chapterEditor.value?.focus({preventScroll: true})
    const bounds = stage.value?.getBoundingClientRect()
    if (bounds && !fullscreen.value && (bounds.top < 68 || bounds.bottom > window.innerHeight - 12)) {
        stage.value?.scrollIntoView({block: 'start'})
    }
}

function closeChapterEditor(restoreFocus = true) {
    editStart.value = undefined
    if (restoreFocus) {
        const trigger = editTrigger?.isConnected ? editTrigger : stage.value
        trigger?.focus({preventScroll: true})
    }
    editTrigger = undefined
}

function cancelChapterEdit() {
    if (!chapterSaving.value) closeChapterEditor()
}

function previewCut() {
    if (!canPreviewCut.value) return
    pause()
    seekToCut(editDraft.value)
}

function updateEditTime(startMs: number) {
    editIssue.value = undefined
    editTime.value = chapterTime(startMs)
}

function nudgeCut(delta: number) {
    const startMs = editDraft.value + delta
    if (!validCut(startMs, otherChapters.value)) return
    updateEditTime(startMs)
    previewCut()
}

function usePlayhead() {
    if (!canUsePlayhead.value || !video.value) return
    pause()
    mediaTime()
    if (canUsePlayhead.value) updateEditTime(playheadMs.value)
}

async function saveChapterEdit() {
    editIssue.value = undefined
    const original = chapters.value.find(chapter => chapter.startMs === editStart.value)
    if (!original) return
    const startMs = editDraft.value
    if (startMs !== original.startMs && !validCut(startMs, otherChapters.value)) {
        editIssue.value = 'chapterTimeInvalid'
        return
    }
    const title = editTitle.value.trim()
    if ([...title].length > maxChapterTitle || /[\u0000-\u001f\u007f-\u009f]/.test(title)) {
        editIssue.value = 'chapterTitleInvalid'
        return
    }
    if (await commitChapters([...otherChapters.value, {startMs, title}].sort((a, b) => a.startMs - b.startMs))) {
        closeChapterEditor()
    }
}

async function removeChapter() {
    if (editStart.value === 0 && chapters.value.length !== 1) return
    if (await commitChapters(otherChapters.value)) {
        closeChapterEditor()
    }
}

function retryChapters() {
    if (editStart.value !== undefined) closeChapterEditor()
    reloadChapters()
}

watch([directory, lookupName], async ([dir, name], _, onCleanup) => {
    unlisted.value = undefined
    findError.value = ''
    findingFile.value = !!name
    if (!name) return

    const controller = new AbortController()
    onCleanup(() => controller.abort())
    try {
        const item = await findUnlistedFile(dir, name, controller.signal)
        if (!controller.signal.aborted) unlisted.value = item
    } catch (cause) {
        if (!controller.signal.aborted) findError.value = (cause as Error).message
    } finally {
        if (!controller.signal.aborted) findingFile.value = false
    }
}, {immediate: true})

async function play() {
    playRequested = true
    const generation = playbackGeneration
    try {
        await video.value?.play()
    } catch (cause) {
        if (generation !== playbackGeneration) return
        if (cause instanceof DOMException && ['NotAllowedError', 'AbortError'].includes(cause.name)) return
        onMediaError()
    }
}

function pause() {
    playRequested = false
    video.value?.pause()
}

function unloadPlayback() {
    playbackGeneration++
    compatibilityProbe.value?.abort()
    compatibilityProbe.value = undefined
    hls?.destroy()
    hls = undefined
    unloadMedia(video.value)
    compatible.value = false
    waitingForPicture.value = false
}

async function playOriginal() {
    const position = video.value?.currentTime ?? 0
    unloadPlayback()
    playRequested = true
    const generation = playbackGeneration
    failure.value = undefined
    directFailed.value = false
    await nextTick()
    const media = video.value
    if (generation !== playbackGeneration || !media) return
    media.currentTime = position
    setSpeed()
    if (playRequested) void play()
}

function compatibilityFailed(reason: CompatibilityFailure) {
    waitingForPicture.value = false
    failure.value = reason
    pause()
}

function compatibilityProbeFailed(media: HTMLVideoElement, status: number | undefined, requestedByUser: boolean) {
    const playable = !media.error && media.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA
    if (status === 422) {
        audioOnly.value = true
        return
    }
    // 浏览器可能只解出了音轨。自动探测失败时保留手动入口。
    if (!requestedByUser && status === 503 && playable && !media.videoWidth) return
    failure.value = transcodeError(status)
}

async function startCompatible(requestedByUser = false) {
    const media = video.value
    if (!src.value || !media || compatible.value || compatibilityProbe.value || audioOnly.value) return
    const url = conversionUrl(src.value, 'transcode')
    let generation = playbackGeneration
    const probe = new AbortController()
    compatibilityProbe.value = probe
    if (requestedByUser) playRequested = true
    attemptedCompatibility = true
    failure.value = undefined

    try {
        const {default: HlsPlayer} = await import('hls.js/light')
        if (generation !== playbackGeneration) return
        const useHls = HlsPlayer.isSupported()
        if (!useHls && !media.canPlayType('application/vnd.apple.mpegurl')) {
            failure.value = 'transcodeUnsupported'
            return
        }

        let trackDuration = 0
        if (useHls) {
            // 每次加载都可能启动转码，失败后由用户决定是否再试。
            const conversionLoadPolicy = {
                default: {
                    maxTimeToFirstByteMs: 50_000,
                    maxLoadTimeMs: 50_000,
                    timeoutRetry: null,
                    errorRetry: null,
                },
            }
            const player = new HlsPlayer({
                autoStartLoad: false,
                maxBufferLength: 12,
                backBufferLength: 16,
                manifestLoadPolicy: conversionLoadPolicy,
                fragLoadPolicy: conversionLoadPolicy,
            })
            hls = player
            // 先加载播放器自己的清单，成功后才替换原片。
            const duration = await new Promise<number | undefined>((resolve) => {
                probe.signal.addEventListener('abort', () => resolve(undefined), {once: true})
                player.once(HlsPlayer.Events.MANIFEST_PARSED, (_, {levels, firstLevel}) => {
                    resolve(levels[firstLevel]?.details?.totalduration ?? 0)
                })
                player.on(HlsPlayer.Events.ERROR, (_, details) => {
                    if (generation !== playbackGeneration || !details.fatal) return
                    const status = details.response?.code
                    if (compatible.value) compatibilityFailed(transcodeError(status))
                    else {
                        compatibilityProbeFailed(media, status, requestedByUser)
                        resolve(undefined)
                    }
                })
                player.loadSource(url)
            })
            if (duration === undefined) return
            trackDuration = duration
        } else {
            const response = await fetch(url, {method: 'HEAD', signal: probe.signal})
            if (generation !== playbackGeneration) return
            if (!response.ok) {
                compatibilityProbeFailed(media, response.status, requestedByUser)
                return
            }
        }
        if (generation !== playbackGeneration || failure.value || audioOnly.value) return
        const position = media.currentTime
        generation = ++playbackGeneration
        directFailed.value = false
        compatible.value = true
        waitingForPicture.value = true
        playing.value = false
        await nextTick()
        if (generation !== playbackGeneration) return
        media.load()

        if (hls) {
            hls.attachMedia(media)
            hls.startLoad(position < trackDuration ? position : 0)
        } else {
            media.addEventListener('loadedmetadata', () => {
                if (generation === playbackGeneration && position < media.duration) media.currentTime = position
            }, {once: true})
            media.src = url
        }
        setSpeed()
        if (playRequested) void play()
    } catch {
        if (generation !== playbackGeneration) return
        if (compatible.value) compatibilityFailed('transcodeFailed')
        else failure.value = 'transcodeFailed'
    } finally {
        if (compatibilityProbe.value === probe) {
            compatibilityProbe.value = undefined
            if (!compatible.value) {
                hls?.destroy()
                hls = undefined
            }
        }
    }
}

function transcodeError(status?: number): CompatibilityFailure {
    if (status === 503) return 'transcodeUnavailable'
    if (status === 409) return 'sourceChanged'
    return 'transcodeFailed'
}

function onMediaError() {
    if (!src.value) return
    if (compatible.value) {
        if (video.value?.error) compatibilityFailed('transcodeFailed')
        return
    }
    directFailed.value = true
    if (!attemptedCompatibility) void startCompatible()
}

function onMediaData() {
    const media = video.value
    if (!media) return
    if (compatible.value) {
        if (media.videoWidth) waitingForPicture.value = false
        return
    }
    directFailed.value = false
    directPicture.value = media.videoWidth > 0
    if (!media.videoWidth && !attemptedCompatibility) void startCompatible()
}

function togglePlayback() {
    if (video.value?.paused) void play()
    else pause()
}

function mediaTime() {
    if (!video.value) return
    time.value = video.value.currentTime
    duration.value = Number.isFinite(video.value.duration) ? video.value.duration : 0
}

function setTime(event: Event) {
    if (video.value && duration.value > 0) video.value.currentTime = (event.target as HTMLInputElement).valueAsNumber
}

function mediaVolume() {
    if (!video.value) return
    volume.value = video.value.volume
    muted.value = video.value.muted
}

function setVolume(event: Event) {
    if (!video.value) return
    video.value.volume = (event.target as HTMLInputElement).valueAsNumber
    video.value.muted = false
}

function toggleMute() {
    if (video.value) video.value.muted = !video.value.muted
}

function setSpeed() {
    if (video.value) video.value.playbackRate = speed.value
}

function select(at: number) {
    const item = playlist.value[at]
    if (item && at !== index.value) void router.replace(playerRoute(directory.value, item.name))
}

function ended() {
    if (!src.value || !video.value?.ended) return
    playing.value = false
    if (editStart.value === undefined) select(index.value + 1)
}

function updateFullscreen() {
    fullscreen.value = document.fullscreenElement === stage.value
}

async function toggleFullscreen() {
    if (fullscreen.value) await document.exitFullscreen()
    else await stage.value?.requestFullscreen()
}

async function togglePip() {
    if (document.pictureInPictureElement) await document.exitPictureInPicture()
    else await video.value?.requestPictureInPicture()
}

function playerKeys(event: KeyboardEvent) {
    if (event.target !== stage.value || !video.value || event.altKey || event.metaKey || event.ctrlKey) return
    if (event.key === ' ') {
        event.preventDefault()
        togglePlayback()
    } else if (duration.value > 0 && ['ArrowLeft', 'ArrowRight'].includes(event.key)) {
        event.preventDefault()
        const delta = event.key === 'ArrowLeft' ? -5 : 5
        video.value.currentTime = Math.max(0, Math.min(duration.value, video.value.currentTime + delta))
    } else if (event.code === 'KeyC' && canSplit.value && !event.repeat) {
        event.preventDefault()
        void splitChapter()
    }
}

function setSleepTimer() {
    window.clearTimeout(timer)
    if (sleepMinutes.value === 0) return
    timer = window.setTimeout(() => {
        pause()
        sleepMinutes.value = 0
    }, sleepMinutes.value * 60 * 1000)
}

watch(src, async (source, _, onCleanup) => {
    let superseded = false
    onCleanup(() => { superseded = true })
    chapterNotice.value = undefined
    closeChapterEditor(false)
    chapterPicker.value?.close()
    playRequested = playMode.value === 'automatic'
    attemptedCompatibility = false
    audioOnly.value = false
    directPicture.value = undefined
    failure.value = undefined
    directFailed.value = false
    playing.value = false
    time.value = 0
    duration.value = 0
    await nextTick()
    if (superseded || !source || !video.value) return
    video.value.volume = volume.value
    video.value.muted = muted.value
    setSpeed()
    if (playRequested) void play()
}, {immediate: true})

watch([index, railTab], async () => {
    await nextTick()
    const container = queue.value
    const item = container?.querySelector<HTMLElement>('[aria-current="true"]')
    if (!container || !item) return
    const containerBounds = container.getBoundingClientRect()
    const itemBounds = item.getBoundingClientRect()
    if (itemBounds.top < containerBounds.top || itemBounds.bottom > containerBounds.bottom) {
        container.scrollTop += itemBounds.top - containerBounds.top - 8
    }
}, {flush: 'post', immediate: true})

onMounted(() => document.addEventListener('fullscreenchange', updateFullscreen))
onBeforeUnmount(() => {
    unloadPlayback()
    window.clearTimeout(timer)
    document.removeEventListener('fullscreenchange', updateFullscreen)
})

const supportsFullscreen = document.fullscreenEnabled
const supportsPip = document.pictureInPictureEnabled
</script>

<template>
    <section class="player-page">
        <RouterLink :to="backToLibrary" class="back-link"><ArrowLeft :size="16" />{{ t('backToLibrary') }}</RouterLink>
        <div v-if="error || findError" class="status-panel" role="alert">
            <CircleAlert :size="29" /><strong>{{ t('loadFailed') }}</strong><p>{{ error || findError }}</p>
            <button class="button" type="button" @click="reload">{{ t('retry') }}</button>
        </div>
        <div v-else-if="(pending || findingFile) && !current" class="status-panel" role="status">
            <LoaderCircle :size="28" /><strong>{{ t('loading') }}</strong>
        </div>
        <div v-else-if="!current" class="status-panel">
            <Film :size="30" /><strong>{{ t('mediaMissing') }}</strong>
            <RouterLink class="button" :to="backToLibrary">{{ t('backToLibrary') }}</RouterLink>
        </div>
        <template v-else>
            <header class="player-heading">
                <h1 :title="current.name">{{ title(current.name) }}</h1>
                <p>
                    <span v-if="extension(current.name)" class="format">{{ extension(current.name).toUpperCase() }}</span>
                    <span v-if="current.size !== undefined">{{ formatSize(current.size) }}</span>
                </p>
            </header>

            <div class="player-columns">
                <div class="player-main">
                    <div ref="stage" class="stage" :class="{'editing-chapter': editStart !== undefined}"
                        tabindex="0" :aria-label="t('player')" role="region" @keydown="playerKeys">
                        <div class="picture" :class="{'audio-picture': audioLayout}">
                            <video ref="video" :src="compatible ? undefined : src || undefined" playsinline preload="metadata"
                                @click="togglePlayback" @durationchange="mediaTime" @timeupdate="mediaTime"
                                @volumechange="mediaVolume" @play="playing = true" @pause="playing = false"
                                @ended="ended" @loadeddata="onMediaData" @error="onMediaError"></video>
                            <div v-if="audioLayout" class="audio-art" aria-hidden="true"><Music2 :size="58" :stroke-width="1.3" /></div>
                            <div v-if="mediaLoading" class="play-error" role="status">
                                <LoaderCircle :size="28" /><strong>{{ t('preparingMedia') }}</strong>
                            </div>
                            <div v-else-if="mediaLoadFailed || failure === 'sourceChanged'" class="play-error" role="alert">
                                <CircleAlert :size="32" /><strong>{{ t(mediaLoadFailed ? 'mediaLoadFailed' : 'sourceChanged') }}</strong>
                                <button class="button" type="button" @click="reloadMedia">{{ t('retry') }}</button>
                            </div>
                            <div v-else-if="preparing" class="play-error" role="status">
                                <LoaderCircle :size="28" /><strong>{{ t('preparingPlayback') }}</strong>
                            </div>
                            <div v-else-if="playbackBlocked" class="play-error" role="alert">
                                <CircleAlert :size="32" /><strong>{{ t(failure ?? 'mediaError') }}</strong>
                                <div class="play-error-actions">
                                    <button class="button" type="button" @click="reloadMedia">{{ t('retry') }}</button>
                                    <a class="button" :href="fileSource" :download="current.name"><Download />{{ t('download') }}</a>
                                </div>
                            </div>
                            <button v-else-if="!playing" type="button" class="play-overlay" :aria-label="t('play')" @click="play">
                                <Play :size="32" fill="currentColor" />
                            </button>
                        </div>
                        <div class="controls">
                            <div class="timeline" :class="{'has-chapters': chapterSegments.length}">
                                <div v-if="chapterSegments.length" class="chapter-track" aria-hidden="true">
                                    <span v-for="segment in chapterSegments" :key="segment.start"
                                        :style="{width: segment.width, '--fill': segment.fill}"></span>
                                </div>
                                <input class="seek" type="range" min="0" :max="duration || 1" step="0.05" :value="time"
                                    :disabled="!duration" :aria-label="t('seek')" :aria-valuetext="formatTime(time) + ' / ' + formatTime(duration)"
                                    @input="setTime">
                            </div>
                            <div class="control-row">
                                <button class="control" type="button" :disabled="index <= 0" :aria-label="t('previous')"
                                    :title="t('previous')" @click="select(index - 1)"><SkipBack :size="19" fill="currentColor" /></button>
                                <button class="control primary-control" type="button" :disabled="!src || playbackBlocked"
                                    :aria-label="t(playing ? 'pause' : 'play')" :title="t(playing ? 'pause' : 'play')" @click="togglePlayback">
                                    <component :is="playing ? Pause : Play" :size="19" fill="currentColor" />
                                </button>
                                <button class="control" type="button" :disabled="index >= playlist.length - 1" :aria-label="t('next')"
                                    :title="t('next')" @click="select(index + 1)"><SkipForward :size="19" fill="currentColor" /></button>
                                <span class="time">{{ formatTime(time) }} <span>/ {{ formatTime(duration) }}</span></span>
                                <div class="control-spacer"></div>
                                <button class="control" type="button" :aria-label="t(muted ? 'unmute' : 'mute')"
                                    :title="t(muted ? 'unmute' : 'mute')" @click="toggleMute">
                                    <component :is="muted || !volume ? VolumeX : Volume2" :size="20" />
                                </button>
                                <input class="volume" type="range" min="0" max="1" step="0.05" :value="muted ? 0 : volume"
                                    :aria-label="t('volume')" @input="setVolume">
                                <select v-model.number="speed" class="speed" :aria-label="t('speed')" :title="t('speed')" @change="setSpeed">
                                    <option v-for="rate in [0.5, 1, 1.25, 1.5, 2, 3]" :key="rate" :value="rate">{{ rate }}×</option>
                                </select>
                                <button v-if="supportsPip && !audioLayout" class="control pip" type="button" :disabled="playbackBlocked || preparing || !duration"
                                    :aria-label="t('pictureInPicture')" :title="t('pictureInPicture')" @click="togglePip">
                                    <PictureInPicture2 :size="19" />
                                </button>
                                <button v-if="supportsFullscreen" class="control" type="button"
                                    :aria-label="t(fullscreen ? 'exitFullscreen' : 'fullscreen')"
                                    :title="t(fullscreen ? 'exitFullscreen' : 'fullscreen')" @click="toggleFullscreen">
                                    <component :is="fullscreen ? Minimize : Maximize" :size="19" />
                                </button>
                            </div>
                        </div>
                        <div v-if="chapters.length || canWrite || chapterIssue" class="chapter-toolbar">
                            <button class="chapter-tool chapter-picker-button" type="button" aria-haspopup="dialog"
                                @click="openChapterPicker">
                                <ListTree :size="17" />
                                <span>{{ t('chapters') }}</span>
                                <span v-if="chapters.length" class="chapter-count" aria-hidden="true">{{ chapters.length }}</span>
                            </button>
                            <span v-if="currentChapter" class="chapter-current">{{ chapterTitle(currentChapter, activeChapter) }}</span>
                            <button v-if="canWrite" class="chapter-tool split-tool" type="button" :disabled="!canSplit"
                                aria-keyshortcuts="C" :title="t('chapterSplit') + ' (C)'" @click="splitChapter">
                                <Scissors :size="16" />{{ t(chapterSaving ? 'chapterSaving' : 'chapterSplit') }}
                            </button>
                        </div>
                        <div v-if="chapterNotice || chapterIssue" class="chapter-feedback" role="status">
                            <span v-if="chapterNotice">{{ t(chapterNotice.key, {time: chapterNotice.time}) }}</span>
                            <span v-else-if="chapterIssue">{{ t(chapterIssue) }}</span>
                            <button v-if="chapterIssue === 'chapterConflict' || chapterIssue === 'chapterLoadFailed'" class="chapter-tool"
                                type="button" @click="retryChapters">{{ t('chapterReload') }}</button>
                        </div>

                        <dialog ref="chapterPicker" class="chapter-dialog chapter-picker" aria-labelledby="chapter-picker-title"
                            @click.self="chapterPicker?.close()" @close="pickerOpen = !!chapterPicker?.open">
                            <div class="chapter-dialog-body">
                                <header class="chapter-dialog-heading">
                                    <h2 id="chapter-picker-title"><ListTree :size="19" />{{ t('chapters') }}</h2>
                                    <button class="control" type="button" :aria-label="t('chapterClose')" @click="chapterPicker?.close()">
                                        <X :size="19" />
                                    </button>
                                </header>
                                <ChapterList v-if="pickerOpen" :chapters="chapters" :active="activeChapter" :duration="duration"
                                    :editable="canEditChapters && !chapterSaving" :loading="chapterLoading"
                                    @select="selectChapter" @edit="openChapterEditor" />
                                <div v-if="chapterNotice || chapterIssue" class="chapter-feedback" role="status">
                                    <span v-if="chapterNotice">{{ t(chapterNotice.key, {time: chapterNotice.time}) }}</span>
                                    <span v-else-if="chapterIssue">{{ t(chapterIssue) }}</span>
                                    <button v-if="chapterIssue === 'chapterConflict' || chapterIssue === 'chapterLoadFailed'" class="chapter-tool"
                                        type="button" @click="retryChapters">{{ t('chapterReload') }}</button>
                                </div>
                                <footer v-if="canWrite" class="chapter-picker-footer">
                                    <button class="button" type="button" :disabled="!canSplit" @click="splitChapter">
                                        <Scissors :size="16" />{{ t(chapterSaving ? 'chapterSaving' : 'chapterSplit') }}
                                    </button>
                                </footer>
                            </div>
                        </dialog>

                        <form v-if="editStart !== undefined" ref="chapterEditor" class="chapter-editor"
                            tabindex="-1" aria-labelledby="chapter-editor-title" @submit.prevent="saveChapterEdit"
                            @keydown.esc.stop.prevent="cancelChapterEdit">
                            <header class="chapter-dialog-heading">
                                <div class="chapter-editor-heading">
                                    <h2 id="chapter-editor-title">{{ t('chapterEdit') }}</h2>
                                    <span>{{ t('chapterOriginalStart', {time: chapterTime(editStart)}) }}</span>
                                </div>
                                <div class="chapter-editor-commands">
                                    <button v-if="chapterIssue === 'chapterConflict'" class="button" type="button"
                                        @click="retryChapters">{{ t('chapterReload') }}</button>
                                    <button v-else class="button primary" type="submit" :disabled="chapterSaving">
                                        {{ t(chapterSaving ? 'chapterSaving' : 'chapterSave') }}
                                    </button>
                                    <button class="control" type="button" :aria-label="t('chapterClose')" :disabled="chapterSaving"
                                        @click="cancelChapterEdit">
                                        <X :size="19" />
                                    </button>
                                </div>
                            </header>
                            <fieldset class="chapter-edit-fields" :disabled="chapterSaving || chapterIssue === 'chapterConflict'">
                                <label for="chapter-edit-time">{{ t('chapterStart') }}</label>
                                <div class="chapter-adjust" :class="{'fixed-cut': editStart === 0}">
                                    <template v-for="(delta, index) in editSteps" :key="delta">
                                        <input v-if="index === 2" id="chapter-edit-time" v-model="editTime" type="text"
                                            :readonly="editStart === 0" placeholder="1:23.500" @input="editIssue = undefined">
                                        <button v-if="editStart > 0" class="button chapter-step" type="button"
                                            :aria-label="t(delta < 0 ? 'chapterEarlier' : 'chapterLater', {seconds: Math.abs(delta) / 1000})"
                                            :disabled="!canSeek || !validCut(editDraft + delta, otherChapters)" @click="nudgeCut(delta)">
                                            {{ delta < 0 ? '−' : '+' }}{{ Math.abs(delta) / 1000 }}
                                        </button>
                                    </template>
                                </div>
                                <div v-if="editStart > 0" class="chapter-time-actions">
                                    <button class="button quiet" type="button" :disabled="!canPreviewCut" @click="previewCut">
                                        <Crosshair :size="16" />{{ t('chapterPreviewCut') }}
                                    </button>
                                    <button class="button quiet" type="button" :disabled="!canUsePlayhead" @click="usePlayhead">
                                        {{ t('chapterUsePlayhead') }}
                                    </button>
                                </div>
                                <label for="chapter-edit-name">{{ t('chapterTitle') }}</label>
                                <input id="chapter-edit-name" v-model="editTitle" :maxlength="maxChapterTitle * 2" type="text"
                                    :placeholder="t('chapterNumber', {number: editIndex + 1})">
                            </fieldset>
                            <p v-if="editIssue || chapterIssue" class="chapter-feedback" role="alert">{{ t(editIssue ?? chapterIssue!) }}</p>
                            <div v-if="editStart > 0 || chapters.length === 1" class="chapter-edit-actions">
                                <button class="button quiet" type="button"
                                    :disabled="chapterSaving || chapterIssue === 'chapterConflict'" @click="removeChapter">
                                    {{ t(editStart > 0 ? 'chapterMerge' : 'chapterRemove') }}
                                </button>
                            </div>
                        </form>
                    </div>

                    <div class="play-preferences">
                        <button v-if="!audioOnly" class="button quiet" type="button" :aria-pressed="compatible"
                            :disabled="!compatible && !!compatibilityProbe"
                            @click="compatible ? playOriginal() : startCompatible(true)">
                            <Film :size="17" />{{ t(compatible ? 'originalPlayback' : 'compatiblePlayback') }}
                        </button>
                        <label class="play-pref">
                            <Repeat2 :size="17" /><span class="sr-only">{{ t('playMode') }}</span>
                            <select v-model="playMode" class="select">
                                <option value="automatic">{{ t('automatic') }}</option>
                                <option value="manual">{{ t('manual') }}</option>
                            </select>
                        </label>
                        <label class="play-pref">
                            <Clock3 :size="17" /><span>{{ t('sleepTimer') }}</span>
                            <select v-model.number="sleepMinutes" class="select" @change="setSleepTimer">
                                <option :value="0">{{ t('sleepOff') }}</option>
                                <option v-for="minutes in [20, 30, 40, 60]" :key="minutes" :value="minutes">
                                    {{ t('sleepAfter', {minutes}) }}
                                </option>
                            </select>
                        </label>
                    </div>
                    <p v-if="failure && !playbackBlocked" class="play-notice" role="status">{{ t(failure) }}</p>
                </div>

                <aside class="queue" :aria-label="t(railTab)">
                    <header class="queue-heading">
                        <button class="rail-tab" type="button" :aria-pressed="railTab === 'queue'" @click="railTab = 'queue'">
                            <ListVideo :size="17" />{{ t('queue') }}
                        </button>
                        <button class="rail-tab" type="button" :aria-pressed="railTab === 'chapters'" @click="railTab = 'chapters'">
                            <ListTree :size="17" />{{ t('chapters') }}
                        </button>
                        <span v-if="railTab === 'queue'" class="queue-progress">{{ index + 1 }} / {{ playlist.length }}</span>
                        <span v-else-if="chapters.length" class="queue-progress">{{ chapters.length }}</span>
                    </header>
                    <ChapterList v-if="railTab === 'chapters'" class="queue-items" :chapters="chapters"
                        :active="activeChapter" :duration="duration" :editable="canEditChapters && !chapterSaving"
                        :loading="chapterLoading" @select="selectChapter" @edit="openChapterEditor" />
                    <ol v-else ref="queue" class="queue-items">
                        <li v-for="(item, position) in playlist" :key="item.name">
                            <RouterLink class="queue-link" :class="{'current-item': position === index}" replace
                                :aria-current="position === index ? 'true' : undefined"
                                :to="playerRoute(directory, item.name)" :title="item.name">
                                <MediaCover v-if="item.type === 'file' || pictureAvailable"
                                    class="queue-cover" :src="mediaUrl(directory, item)"
                                    :audio="isAudio(item.name) && !(position === index && pictureAvailable)" />
                                <span v-else class="queue-cover other-cover"><Film :size="18" aria-hidden="true" /></span>
                                <div class="queue-copy">
                                    <strong>{{ title(item.name) }}</strong>
                                    <span v-if="position === index" class="now-playing">
                                        <component :is="playing ? Pause : Play" :size="12" fill="currentColor" />{{ t('nowPlaying') }}
                                    </span>
                                    <span v-else>{{ extension(item.name).toUpperCase() }}<template v-if="item.size !== undefined">
                                        · {{ formatSize(item.size) }}</template></span>
                                </div>
                            </RouterLink>
                        </li>
                    </ol>
                </aside>
            </div>
        </template>
    </section>
</template>

<style scoped>
.player-page { display: grid; gap: 17px; }
.back-link { display: inline-flex; align-items: center; justify-self: start; gap: 8px; color: var(--muted); font-size: 13px; }
.back-link:hover { color: var(--accent); }
.player-heading { display: grid; min-width: 0; gap: 10px; padding-bottom: 4px; }
.player-heading h1 { overflow-wrap: anywhere; font-size: clamp(23px, 3.1vw, 37px); line-height: 1.16; font-weight: 710; letter-spacing: -.035em; }
.player-heading p { display: flex; align-items: center; gap: 10px; color: var(--muted); font-size: 13px; }
.format { padding: 3px 6px; border: 1px solid var(--border); border-radius: 5px; background: var(--panel); color: var(--text); font-size: 10px; font-weight: 670; letter-spacing: .06em; }

.player-columns { display: grid; grid-template-columns: minmax(0, 1fr) minmax(275px, 32%); gap: 18px; align-items: stretch; }
.player-main { display: grid; gap: 13px; align-content: start; min-width: 0; }
.stage { display: flex; flex-direction: column; overflow: hidden; border: 1px solid var(--border); border-radius: 13px; background: #090a0e; }
.picture { display: grid; place-items: center; position: relative; isolation: isolate; aspect-ratio: 16 / 9; background: #06070a; }
.picture video { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: contain; cursor: pointer; }
.audio-picture { background: radial-gradient(ellipse at 67% 20%, #3b334d, transparent 70%), linear-gradient(130deg, #232c3d, #1d1825); }
.audio-picture video { opacity: 0; }
.audio-art { display: grid; place-items: center; width: 104px; height: 104px; border: 1px solid #dbbfe752; border-radius: 24px; color: #dfc3e8; background: #8e739d23; pointer-events: none; }
.play-overlay { position: absolute; display: grid; place-items: center; width: clamp(48px, 6vw, 67px); height: clamp(48px, 6vw, 67px); border: 1px solid #ffffff40; border-radius: 50%; background: #10131be6; color: #fff; transition: background 160ms, color 160ms; }
.play-overlay svg { width: clamp(23px, 3vw, 32px); margin-left: 3px; }
.play-overlay:hover { background: var(--accent); color: #221517; }
.play-error { position: absolute; inset: 0; display: grid; place-content: center; justify-items: center; gap: 11px; padding: 10px; background: #08090de8; text-align: center; }
.play-error > svg { color: var(--accent); }
.play-error-actions { display: flex; justify-content: center; flex-wrap: wrap; gap: 8px; }
.play-notice { color: var(--muted); font-size: 12px; }

.controls { padding: 3px 12px 7px; background: #191c23; }
.seek, .volume { display: block; margin: 0; accent-color: var(--accent); cursor: pointer; }
.seek { width: 100%; height: 17px; }
.timeline { position: relative; display: grid; align-items: center; min-height: 22px; }
.chapter-track { position: absolute; inset-inline: 6px; display: flex; height: 5px; pointer-events: none; }
.chapter-track span { min-width: 0; border-radius: 3px; background: linear-gradient(to right, var(--accent) var(--fill), #494e5c var(--fill)); }
.chapter-track span:not(:last-child) { box-shadow: inset -2px 0 #191c23; }
.has-chapters .seek { position: relative; appearance: none; background: transparent; }
.has-chapters .seek::-webkit-slider-runnable-track { height: 14px; background: none; }
.has-chapters .seek::-moz-range-track { height: 14px; background: none; }
.has-chapters .seek::-webkit-slider-thumb { appearance: none; width: 13px; height: 13px; border: 2px solid #191c23; border-radius: 50%; background: var(--accent); }
.has-chapters .seek::-moz-range-thumb { width: 10px; height: 10px; border: 2px solid #191c23; border-radius: 50%; background: var(--accent); }
.control-row { display: flex; align-items: center; gap: 4px; min-width: 0; }
.control { display: grid; place-items: center; width: 34px; height: 34px; padding: 0; border: 0; border-radius: 6px; background: none; color: var(--text); }
.control:hover:not(:disabled), .speed:hover { background: var(--raised); }
.primary-control { color: var(--accent); }
.control-spacer { flex: 1; }
.time { margin: 0 5px; color: var(--text); font-size: 11px; white-space: nowrap; font-variant-numeric: tabular-nums; }
.time span { color: var(--muted); }
.volume { width: clamp(35px, 5vw, 68px); }
.speed { min-height: 31px; padding: 0 4px; border: 0; border-radius: 6px; background: transparent; color: var(--text); font-size: 12px; }
.stage:fullscreen { border: 0; border-radius: 0; }
.stage:fullscreen .picture { flex: 1; min-height: 0; aspect-ratio: auto; }
.stage.editing-chapter { scroll-margin-top: 68px; }
.editing-chapter:not(:fullscreen) .picture { max-height: 34dvh; }

.chapter-toolbar { display: flex; align-items: center; gap: 9px; min-width: 0; padding: 4px 12px; border-top: 1px solid var(--border); background: #191c23; }
.chapter-tool { display: inline-flex; align-items: center; gap: 6px; min-height: 34px; padding: 0 9px; border: 0; border-radius: 6px; background: transparent; color: var(--text); font-size: 12px; font-weight: 600; white-space: nowrap; }
.chapter-tool:hover:not(:disabled) { background: var(--raised); }
.chapter-tool svg { color: var(--accent); }
.chapter-count { color: var(--muted); font-size: 10px; font-variant-numeric: tabular-nums; }
.chapter-current { min-width: 0; overflow: hidden; color: var(--muted); font-size: 12px; text-overflow: ellipsis; white-space: nowrap; }
.split-tool { margin-left: auto; background: var(--accent-soft); color: var(--accent); }
.split-tool:hover:not(:disabled) { background: #ff796b30; }
.chapter-feedback { display: flex; align-items: center; justify-content: space-between; gap: 6px; padding: 4px 13px 7px; color: var(--accent); font-size: 12px; line-height: 1.5; }
.chapter-feedback .chapter-tool { min-height: 28px; }
.chapter-dialog { width: min(420px, calc(100% - 24px)); max-height: min(520px, 85dvh); padding: 0; overflow: hidden; border: 1px solid var(--border); border-radius: 14px; background: var(--panel); color: var(--text); box-shadow: 0 18px 70px #0008; }
.chapter-dialog::backdrop { background: #07090cac; }
.chapter-dialog-body { display: flex; flex-direction: column; max-height: min(518px, calc(85dvh - 2px)); }
.chapter-dialog-heading { display: flex; align-items: center; justify-content: space-between; gap: 10px; padding: 8px 12px; border-bottom: 1px solid var(--border); }
.chapter-dialog-heading h2 { display: flex; align-items: center; gap: 8px; font-size: 16px; }
.chapter-dialog-heading h2 svg { color: var(--accent); }
.chapter-picker-footer { display: flex; justify-content: flex-end; padding: 10px 12px; border-top: 1px solid var(--border); }
.chapter-editor { flex: 0 0 auto; max-height: 46dvh; overflow: auto; border-top: 1px solid var(--border); background: var(--panel); }
.chapter-editor:focus { outline: none; }
.chapter-editor .chapter-dialog-heading { position: sticky; top: 0; z-index: 1; background: var(--panel); }
.chapter-editor-heading { display: flex; align-items: baseline; flex-wrap: wrap; gap: 3px 9px; }
.chapter-editor-heading span { color: var(--muted); font-size: 12px; font-variant-numeric: tabular-nums; }
.chapter-editor-commands { display: flex; align-items: center; gap: 3px; }
.chapter-editor-commands .button { padding-inline: 10px; }
.chapter-edit-fields { display: grid; gap: 7px; min-inline-size: 0; margin: 0; padding: 10px 12px; border: 0; }
.chapter-editor label { color: var(--muted); font-size: 12px; }
.chapter-editor input { min-width: 0; min-height: 40px; padding: 0 9px; border: 1px solid var(--border); border-radius: 8px; background: var(--bg); color: var(--text); }
.chapter-editor input[readonly] { color: var(--muted); }
.chapter-adjust { display: grid; grid-template-columns: repeat(2, minmax(44px, 1fr)) minmax(70px, 2fr) repeat(2, minmax(44px, 1fr)); gap: 5px; max-width: 395px; }
.chapter-adjust.fixed-cut { grid-template-columns: minmax(0, 125px); }
.chapter-adjust input { text-align: center; font-variant-numeric: tabular-nums; }
.chapter-step { min-height: 44px; padding: 0 3px; font-variant-numeric: tabular-nums; }
.chapter-time-actions { display: flex; flex-wrap: wrap; gap: 5px; padding-bottom: 3px; }
.chapter-time-actions .button { min-height: 36px; padding-inline: 8px; }
.chapter-editor .chapter-feedback { padding-top: 8px; }
.chapter-edit-actions { padding: 0 12px 10px; }
.stage:fullscreen .chapter-editor { max-height: 48dvh; }

.play-preferences { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 14px; }
.play-pref { display: inline-flex; align-items: center; gap: 7px; color: var(--muted); font-size: 12px; }
.play-pref .select { min-height: 35px; padding: 0 8px; font-size: 12px; }
.play-pref svg { color: var(--accent); }

.queue { position: relative; min-height: 315px; overflow: hidden; border: 1px solid var(--border); border-radius: 13px; background: var(--rail); }
.queue-heading { display: flex; align-items: center; gap: 5px; height: 55px; padding: 7px 8px; border-bottom: 1px solid var(--border); }
.rail-tab { display: flex; align-items: center; gap: 6px; min-height: 38px; padding: 0 8px; border: 1px solid transparent; border-radius: 7px; background: none; color: var(--muted); font-size: 12px; font-weight: 630; white-space: nowrap; }
.rail-tab:hover { background: var(--hover); color: var(--text); }
.rail-tab[aria-pressed="true"] { border-color: #ff796b30; background: var(--accent-soft); color: var(--accent); }
.queue-progress { margin-left: auto; padding: 4px 7px; border-radius: 5px; background: var(--raised); color: var(--muted); font-size: 11px; white-space: nowrap; font-variant-numeric: tabular-nums; }
.queue-items { position: absolute; inset: 55px 0 0; overflow: auto; overscroll-behavior: contain; margin: 0; padding: 8px; list-style: none; scrollbar-width: thin; }
.queue-link { display: flex; align-items: center; gap: 10px; min-width: 0; padding: 7px; border: 1px solid transparent; border-radius: 8px; }
.queue-link:hover { background: var(--hover); }
.queue-link.current-item { border-color: #ff796b4d; background: var(--accent-soft); }
.queue-cover { flex: 0 0 104px; border-radius: 6px; }
.other-cover { display: grid; place-items: center; aspect-ratio: 16 / 9; background: var(--raised); color: var(--muted); }
.queue-copy { display: grid; gap: 6px; min-width: 0; }
.queue-copy strong { overflow: hidden; color: var(--text); font-size: 13px; font-weight: 630; text-overflow: ellipsis; white-space: nowrap; }
.queue-copy span { color: var(--muted); font-size: 11px; }
.queue-copy .now-playing { display: flex; align-items: center; gap: 5px; color: var(--accent); font-weight: 620; }

@media (max-width: 1050px) and (min-width: 891px), (max-width: 720px) {
    .player-columns { grid-template-columns: minmax(0, 1fr); gap: 16px; }
    .queue { min-height: 0; }
    .queue-items { position: static; max-height: 52vh; }
}
@media (max-width: 520px) {
    .player-page { gap: 13px; }
    .controls { padding-inline: 8px; }
    .control-row { gap: 1px; }
    .control { width: 30px; min-height: 37px; }
    .control svg { width: 17px; }
    .time { margin: 0 3px; }
    .volume, .pip { display: none; }
    .speed { padding-inline: 2px; }
    .queue-cover { flex-basis: 91px; }
    .chapter-toolbar { padding-inline: 7px; }
    .chapter-tool { min-height: 40px; }
    .chapter-picker { inset: auto 0 0; width: 100%; max-width: none; margin: 0; border-radius: 15px 15px 0 0; }
    .chapter-picker-footer { padding-bottom: max(10px, env(safe-area-inset-bottom)); }
    .chapter-edit-fields, .chapter-edit-actions { padding-inline: 8px; }
    .chapter-editor input { font-size: 16px; padding-inline: 5px; }
}
</style>

<script setup lang="ts">
import {computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch} from 'vue'
import {useRoute, useRouter} from 'vue-router'
import {useI18n} from 'vue-i18n'
import type Hls from 'hls.js'
import {
    ArrowLeft, CircleAlert, Clock3, Download, Film, ListVideo, LoaderCircle, Maximize, Minimize,
    Music2, Pause, PictureInPicture2, Play, Repeat2, SkipBack, SkipForward, Volume2, VolumeX,
} from '@lucide/vue'
import type {ExplorerItem} from '../api'
import MediaCover from '../components/MediaCover.vue'
import {
    conversionUrl, directoryPath, extension, findUnlistedFile, formatSize, isAudio, libraryRoute, mediaUrl,
    orderedFiles, playerRoute, title, useDirectory,
} from '../library'
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
const src = computed(() => current.value ? mediaUrl(directory.value, current.value) : '')
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

function stopCompatible() {
    playbackGeneration++
    compatibilityProbe.value?.abort()
    compatibilityProbe.value = undefined
    hls?.destroy()
    hls = undefined
    compatible.value = false
    waitingForPicture.value = false
}

async function playOriginal() {
    const position = video.value?.currentTime ?? 0
    stopCompatible()
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
    playing.value = false
    select(index.value + 1)
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
    stopCompatible()
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

watch(index, async () => {
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
    stopCompatible()
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
                    <div ref="stage" class="stage" tabindex="0" :aria-label="t('player')" role="region" @keydown="playerKeys">
                        <div class="picture" :class="{'audio-picture': audioLayout}">
                            <video ref="video" :src="compatible ? undefined : src" playsinline preload="metadata"
                                @click="togglePlayback" @durationchange="mediaTime" @timeupdate="mediaTime"
                                @volumechange="mediaVolume" @play="playing = true" @pause="playing = false"
                                @ended="ended" @loadeddata="onMediaData" @error="onMediaError"></video>
                            <div v-if="audioLayout" class="audio-art" aria-hidden="true"><Music2 :size="58" :stroke-width="1.3" /></div>
                            <div v-if="preparing" class="play-error" role="status">
                                <LoaderCircle :size="28" /><strong>{{ t('preparingPlayback') }}</strong>
                            </div>
                            <div v-else-if="playbackBlocked" class="play-error" role="alert">
                                <CircleAlert :size="32" /><strong>{{ t(failure ?? 'mediaError') }}</strong>
                                <a class="button" :href="src" :download="current.name"><Download />{{ t('download') }}</a>
                            </div>
                            <button v-else-if="!playing" type="button" class="play-overlay" :aria-label="t('play')" @click="play">
                                <Play :size="32" fill="currentColor" />
                            </button>
                        </div>
                        <div class="controls">
                            <input class="seek" type="range" min="0" :max="duration || 1" step="0.05" :value="time"
                                :disabled="!duration" :aria-label="t('seek')" :aria-valuetext="formatTime(time) + ' / ' + formatTime(duration)"
                                @input="setTime">
                            <div class="control-row">
                                <button class="control" type="button" :disabled="index <= 0" :aria-label="t('previous')"
                                    :title="t('previous')" @click="select(index - 1)"><SkipBack :size="19" fill="currentColor" /></button>
                                <button class="control primary-control" type="button" :disabled="playbackBlocked"
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

                <aside class="queue" aria-labelledby="queue-title">
                    <header class="queue-heading">
                        <span class="queue-icon"><ListVideo :size="21" /></span>
                        <div><h2 id="queue-title">{{ t('queue') }}</h2><p>{{ t('queueCount', playlist.length) }}</p></div>
                        <span class="queue-progress">{{ index + 1 }} / {{ playlist.length }}</span>
                    </header>
                    <ol ref="queue" class="queue-items">
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
.play-notice { color: var(--muted); font-size: 12px; }

.controls { padding: 3px 12px 7px; background: #191c23; }
.seek, .volume { display: block; margin: 0; accent-color: var(--accent); cursor: pointer; }
.seek { width: 100%; height: 17px; }
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
.stage:fullscreen .picture { flex: 1; aspect-ratio: auto; }

.play-preferences { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 14px; }
.play-pref { display: inline-flex; align-items: center; gap: 7px; color: var(--muted); font-size: 12px; }
.play-pref .select { min-height: 35px; padding: 0 8px; font-size: 12px; }
.play-pref svg { color: var(--accent); }

.queue { position: relative; min-height: 315px; overflow: hidden; border: 1px solid var(--border); border-radius: 13px; background: var(--rail); }
.queue-heading { display: flex; align-items: center; gap: 10px; height: 69px; padding: 11px 13px; border-bottom: 1px solid var(--border); }
.queue-icon { display: grid; place-items: center; width: 38px; height: 38px; border-radius: 8px; background: var(--accent-soft); color: var(--accent); }
.queue-heading h2 { font-size: 16px; font-weight: 680; }
.queue-heading p { margin-top: 4px; color: var(--muted); font-size: 12px; }
.queue-progress { margin-left: auto; padding: 4px 7px; border-radius: 5px; background: var(--raised); color: var(--muted); font-size: 11px; white-space: nowrap; font-variant-numeric: tabular-nums; }
.queue-items { position: absolute; inset: 69px 0 0; overflow: auto; overscroll-behavior: contain; margin: 0; padding: 8px; list-style: none; scrollbar-width: thin; }
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
}
</style>

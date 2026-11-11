<script setup lang="ts">
import {computed, nextTick, onBeforeUnmount, onMounted, ref, watch} from 'vue'
import {useRoute, useRouter} from 'vue-router'
import {useI18n} from 'vue-i18n'
import {
    ArrowLeft, CircleAlert, Clock3, Download, Film, ListVideo, LoaderCircle, Maximize, Minimize,
    Music2, Pause, PictureInPicture2, Play, Repeat2, SkipBack, SkipForward, Volume2, VolumeX,
} from '@lucide/vue'
import MediaCover from '../components/MediaCover.vue'
import {
    directoryPath, extension, formatSize, isAudio, libraryRoute, mediaUrl, orderedFiles,
    playerRoute, title, useDirectory,
} from '../library'
import {formatTime} from '../thumbnails'

const {t} = useI18n()
const route = useRoute()
const router = useRouter()
const directory = computed(() => directoryPath(route.query.dir))
const {items, pending, error, reload} = useDirectory(directory)
const playlist = computed(() => orderedFiles(items.value))
const index = computed(() => typeof route.query.file === 'string'
    ? playlist.value.findIndex((item) => item.name === route.query.file)
    : 0)
const current = computed(() => playlist.value[index.value])
const src = computed(() => current.value ? mediaUrl(directory.value, current.value) : '')
const audio = computed(() => !!current.value && isAudio(current.value.name))

const stage = ref<HTMLElement>()
const video = ref<HTMLVideoElement>()
const queue = ref<HTMLElement>()
const playing = ref(false)
const failed = ref(false)
const duration = ref(0)
const time = ref(0)
const volume = ref(1)
const muted = ref(false)
const speed = ref(1)
const fullscreen = ref(false)
const playMode = ref<'automatic' | 'manual'>('automatic')
const sleepMinutes = ref(0)
let timer: number | undefined

async function play() {
    try {
        await video.value?.play()
    } catch (cause) {
        if (cause instanceof DOMException && ['NotAllowedError', 'AbortError'].includes(cause.name)) return
        failed.value = true
    }
}

function togglePlayback() {
    if (video.value?.paused) void play()
    else video.value?.pause()
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
        video.value?.pause()
        sleepMinutes.value = 0
    }, sleepMinutes.value * 60 * 1000)
}

watch(src, async (source, _, onCleanup) => {
    let superseded = false
    onCleanup(() => { superseded = true })
    failed.value = false
    playing.value = false
    time.value = 0
    duration.value = 0
    await nextTick()
    if (superseded || !source || !video.value) return
    video.value.volume = volume.value
    video.value.muted = muted.value
    setSpeed()
    if (playMode.value === 'automatic') void play()
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
    window.clearTimeout(timer)
    document.removeEventListener('fullscreenchange', updateFullscreen)
})

const supportsFullscreen = document.fullscreenEnabled
const supportsPip = document.pictureInPictureEnabled
</script>

<template>
    <section class="player-page">
        <RouterLink :to="libraryRoute(directory)" class="back-link"><ArrowLeft :size="16" />{{ t('backToLibrary') }}</RouterLink>
        <div v-if="error" class="status-panel" role="alert">
            <CircleAlert :size="29" /><strong>{{ t('loadFailed') }}</strong><p>{{ error }}</p>
            <button class="button" type="button" @click="reload">{{ t('retry') }}</button>
        </div>
        <div v-else-if="pending && !current" class="status-panel" role="status">
            <LoaderCircle :size="28" /><strong>{{ t('loading') }}</strong>
        </div>
        <div v-else-if="!current" class="status-panel">
            <Film :size="30" /><strong>{{ t('mediaMissing') }}</strong>
            <RouterLink class="button" :to="libraryRoute(directory)">{{ t('backToLibrary') }}</RouterLink>
        </div>
        <template v-else>
            <header class="player-heading">
                <h1 :title="current.name">{{ title(current.name) }}</h1>
                <p>
                    <span class="format">{{ extension(current.name).toUpperCase() }}</span>
                    <span v-if="current.size !== undefined">{{ formatSize(current.size) }}</span>
                </p>
            </header>

            <div class="player-columns">
                <div class="player-main">
                    <div ref="stage" class="stage" tabindex="0" :aria-label="t('player')" role="region" @keydown="playerKeys">
                        <div class="picture" :class="{'audio-picture': audio}">
                            <video ref="video" :src="src" playsinline preload="metadata"
                                @click="togglePlayback" @durationchange="mediaTime" @timeupdate="mediaTime"
                                @volumechange="mediaVolume" @play="playing = true" @pause="playing = false"
                                @ended="ended" @error="failed = true"></video>
                            <div v-if="audio" class="audio-art" aria-hidden="true"><Music2 :size="58" :stroke-width="1.3" /></div>
                            <div v-if="failed" class="play-error" role="alert">
                                <CircleAlert :size="32" /><strong>{{ t('mediaError') }}</strong>
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
                                <button class="control primary-control" type="button" :disabled="failed"
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
                                <button v-if="supportsPip && !audio" class="control pip" type="button" :disabled="failed || !duration"
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
                                <MediaCover class="queue-cover" :src="mediaUrl(directory, item)" :audio="isAudio(item.name)" />
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

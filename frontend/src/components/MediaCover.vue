<script setup lang="ts">
import {computed, onBeforeUnmount, onMounted, ref, watch} from 'vue'
import {Film, Music2} from '@lucide/vue'
import {extension} from '../library'
import {formatTime, getPreview} from '../thumbnails'

const props = defineProps<{src: string; audio: boolean}>()
const cover = ref<HTMLElement>()
const visible = ref(false)
const image = ref<string>()
const duration = ref(0)
const audioCover = computed(() => !image.value && (props.audio || extension(props.src.split('?')[0]) === 'ogg'))
let observer: IntersectionObserver | undefined

onMounted(() => {
    if (!cover.value || typeof IntersectionObserver === 'undefined') {
        visible.value = true
        return
    }
    observer = new IntersectionObserver((entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
            visible.value = true
            observer?.disconnect()
        }
    }, {rootMargin: '96px'})
    observer.observe(cover.value)
})

watch([visible, () => props.src, () => props.audio], async ([ready, src, audio], _, onCleanup) => {
    image.value = undefined
    duration.value = 0
    if (!ready) return
    const controller = new AbortController()
    onCleanup(() => controller.abort())
    const preview = await getPreview(src, audio, controller.signal)
    if (controller.signal.aborted || !preview) return
    image.value = preview.image
    duration.value = preview.duration
})

onBeforeUnmount(() => observer?.disconnect())
</script>

<template>
    <span ref="cover" class="media-cover" :class="{'audio-cover': audioCover}">
        <img v-if="image" :src="image" alt="" draggable="false">
        <component :is="audioCover ? Music2 : Film" v-else class="cover-icon" aria-hidden="true" />
        <span v-if="duration > 0 && Number.isFinite(duration)" class="cover-time">{{ formatTime(duration) }}</span>
    </span>
</template>

<style scoped>
.media-cover {
    position: relative;
    display: grid;
    place-items: center;
    min-width: 0;
    aspect-ratio: 16 / 9;
    overflow: hidden;
    isolation: isolate;
    background: radial-gradient(ellipse at 78% 16%, #2c4058, transparent 72%), linear-gradient(130deg, #222b3b, #161c26);
    color: #9aafc6;
}

.audio-cover {
    background: radial-gradient(ellipse at 80% 12%, #55405b, transparent 72%), linear-gradient(130deg, #362d43, #211d2c);
    color: #cdb4d8;
}

.media-cover img {
    position: absolute;
    width: 100%;
    height: 100%;
    object-fit: cover;
}

.cover-icon {
    width: clamp(17px, 19%, 34px);
    height: auto;
    stroke-width: 1.5;
    opacity: .75;
}

.cover-time {
    position: absolute;
    right: 6px;
    bottom: 6px;
    padding: 2px 5px;
    background: #0a0c12df;
    border-radius: 4px;
    color: #fff;
    font-size: 11px;
    line-height: 1.2;
    font-variant-numeric: tabular-nums;
}
</style>

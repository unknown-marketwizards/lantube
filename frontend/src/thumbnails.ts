import {conversionUrl} from './library'

interface Preview {
    image: string | undefined
    duration: number
}

const cache = new Map<string, Preview>()
const localSlot = limitPreviews(2)
const serverSlot = limitPreviews(2)

function limitPreviews(limit: number) {
    const waiting: Array<() => void> = []
    let active = 0
    return async function run<T>(work: () => Promise<T>): Promise<T> {
        if (active < limit) active++
        else await new Promise<void>((resolve) => waiting.push(resolve))
        try {
            return await work()
        } finally {
            const next = waiting.shift()
            if (next) next()
            else active--
        }
    }
}

export async function getPreview(src: string, audio: boolean, signal: AbortSignal): Promise<Preview | undefined> {
    const key = audio ? 'audio:' + src : src
    const cached = cache.get(key)
    if (cached) {
        cache.delete(key)
        cache.set(key, cached)
        return cached
    }

    const local = await localSlot(async () => {
        if (signal.aborted) return
        return cache.get(key) ?? capture(src, audio, signal)
    })
    if (signal.aborted) return
    const saved = cache.get(key)
    if (saved) return saved

    const server = !audio && !local?.image ? await serverSlot(async () => {
        if (signal.aborted) return
        return cache.get(key) ?? serverPreview(src, local?.duration ?? 0, signal)
    }) : undefined
    const preview = server ?? local
    if (signal.aborted) return
    if (preview && (audio || preview.image || server)) {
        cache.set(key, preview)
        if (cache.size > 240) cache.delete(cache.keys().next().value!)
    }
    return preview
}

async function serverPreview(src: string, localDuration: number, signal: AbortSignal): Promise<Preview | undefined> {
    try {
        const response = await fetch(conversionUrl(src, 'thumbnail'), {signal})
        if (response.status === 422) return {image: undefined, duration: localDuration}
        if (!response.ok || response.headers.get('content-type') !== 'image/jpeg') return
        const image = await response.blob()
        const duration = Number.isFinite(localDuration) && localDuration > 0
            ? localDuration
            : Number(response.headers.get('x-media-duration'))
        return await new Promise((resolve) => {
            const reader = new FileReader()
            reader.onload = () => resolve({image: String(reader.result), duration})
            reader.onerror = () => resolve(undefined)
            reader.readAsDataURL(image)
        })
    } catch {
        return undefined
    }
}

function capture(src: string, audio: boolean, signal: AbortSignal): Promise<Preview | undefined> {
    return new Promise((resolve) => {
        const video = document.createElement('video')
        video.preload = audio ? 'metadata' : 'auto'
        video.muted = true
        let settled = false
        let seeking = false

        function finish(preview?: Preview) {
            if (settled) return
            settled = true
            clearTimeout(timeout)
            signal.removeEventListener('abort', cancel)
            video.removeAttribute('src')
            video.load()
            resolve(preview)
        }

        function cancel() {
            finish()
        }

        function frame() {
            if (!video.videoWidth || video.readyState < 2) return
            try {
                const canvas = document.createElement('canvas')
                const scale = Math.min(1, 480 / video.videoWidth, 360 / video.videoHeight)
                canvas.width = Math.max(1, Math.round(video.videoWidth * scale))
                canvas.height = Math.max(1, Math.round(video.videoHeight * scale))
                const context = canvas.getContext('2d')
                context?.drawImage(video, 0, 0, canvas.width, canvas.height)
                finish({image: context ? canvas.toDataURL('image/webp', 0.79) : undefined, duration: video.duration})
            } catch {
                finish({image: undefined, duration: video.duration})
            }
        }

        const timeout = window.setTimeout(cancel, 9000)
        signal.addEventListener('abort', cancel, {once: true})
        video.addEventListener('error', cancel, {once: true})
        video.addEventListener('loadedmetadata', () => {
            if (!video.videoWidth) return finish({image: undefined, duration: video.duration})
            const timestamp = Math.min(60, video.duration * 0.2)
            if (Number.isFinite(timestamp) && timestamp >= 0.2) {
                seeking = true
                video.currentTime = timestamp
            }
        }, {once: true})
        video.addEventListener('loadeddata', () => {
            if (!seeking) frame()
        }, {once: true})
        video.addEventListener('seeked', frame, {once: true})
        video.src = src
    })
}

export function formatTime(seconds: number): string {
    if (!Number.isFinite(seconds) || seconds < 0) return '0:00'
    const total = Math.floor(seconds)
    const minutes = Math.floor(total / 60)
    const remainder = String(total % 60).padStart(2, '0')
    if (minutes < 60) return minutes + ':' + remainder
    return Math.floor(minutes / 60) + ':' + String(minutes % 60).padStart(2, '0') + ':' + remainder
}

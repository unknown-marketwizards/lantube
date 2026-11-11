interface Preview {
    image: string | undefined
    duration: number
}

const cache = new Map<string, Preview>()
const jobs: Array<() => Promise<void>> = []
let active = 0

function runNext() {
    while (active < 2 && jobs.length) {
        const job = jobs.shift()!
        active++
        void job().finally(() => {
            active--
            runNext()
        })
    }
}

export function getPreview(src: string, audio: boolean, signal: AbortSignal): Promise<Preview | undefined> {
    const cached = cache.get(src)
    if (cached) {
        cache.delete(src)
        cache.set(src, cached)
        return Promise.resolve(cached)
    }

    return new Promise((resolve) => {
        jobs.push(async () => {
            if (signal.aborted) return resolve(undefined)
            const preview = cache.get(src) ?? await capture(src, audio, signal)
            if (preview) {
                cache.set(src, preview)
                if (cache.size > 240) cache.delete(cache.keys().next().value!)
            }
            resolve(preview)
        })
        runNext()
    })
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

import {computed, ref, watch, type Ref} from 'vue'

export function unloadMedia(media: HTMLMediaElement | undefined) {
    if (!media) return
    media.pause()
    media.removeAttribute('src')
    media.load()
}

export function useMediaSource(fileSource: Readonly<Ref<string>>, unloadPlayback: () => void) {
    const revision = ref('')
    const loading = ref(false)
    const failed = ref(false)
    const reloadCount = ref(0)
    const src = computed(() => {
        if (!fileSource.value || !revision.value) return ''
        const url = new URL(fileSource.value, location.origin)
        url.searchParams.set('revision', revision.value)
        return url.pathname + url.search
    })

    watch([fileSource, reloadCount], async ([source], _, onCleanup) => {
        unloadPlayback()
        revision.value = ''
        failed.value = false
        loading.value = !!source
        if (!source) return
        const controller = new AbortController()
        onCleanup(() => controller.abort())
        try {
            const response = await fetch(source, {method: 'HEAD', cache: 'no-store', signal: controller.signal})
            if (controller.signal.aborted) return
            const version = response.headers.get('X-Media-Revision')
            if (!response.ok || !version) throw new Error(String(response.status))
            revision.value = version
        } catch {
            if (!controller.signal.aborted) failed.value = true
        } finally {
            if (!controller.signal.aborted) loading.value = false
        }
    }, {immediate: true, flush: 'sync'})

    return {src, revision, loading, failed, reload: () => { reloadCount.value++ }}
}

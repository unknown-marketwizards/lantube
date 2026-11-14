import {computed, ref, shallowRef, watch, type Ref} from 'vue'
import {loadChapters, saveChapters, type Chapter, type ChapterSet} from './api'
import {formatTime} from './thumbnails'

export const minChapterMs = 500
export const maxChapters = 200
export const maxChapterTitle = 120

export function chapterAt(chapters: Chapter[], seconds: number): number {
    return chapters.findLastIndex(chapter => chapter.startMs <= Math.round(seconds * 1000))
}

export function chapterTime(ms: number): string {
    const fraction = String(ms % 1000).padStart(3, '0').replace(/0+$/, '')
    return formatTime(ms / 1000) + (fraction ? '.' + fraction : '')
}

export function parseChapterTime(value: string): number {
    const parts = value.trim().split(':')
    const seconds = parts.pop()!
    if (!/^\d+(\.\d{1,3})?$/.test(seconds) || parts.length > 2 || parts.some(part => !/^\d+$/.test(part))) return NaN
    if (parts.length && (Number(seconds) >= 60 || (parts.length === 2 && Number(parts[1]) >= 60))) return NaN
    return Math.round((parts.reduce((total, part) => total * 60 + Number(part), 0) * 60 + Number(seconds)) * 1000)
}

export function useChapters(path: Ref<string>, revision: Ref<string>, reloadMedia: () => void) {
    const saved = shallowRef<ChapterSet>()
    const chapters = computed(() => saved.value?.chapters ?? [])
    const loading = ref(false)
    const saving = ref(false)
    const issue = ref<'chapterLoadFailed' | 'chapterSaveFailed' | 'chapterConflict' | 'chapterDurationUnavailable' | 'chapterTimeInvalid'>()
    const reloadCount = ref(0)
    let request: AbortController | undefined

    function reload() {
        reloadCount.value++
    }

    watch([path, revision, reloadCount], async ([current, mediaRevision, attempt], [previousPath, previousRevision, previousAttempt], onCleanup) => {
        const controller = new AbortController()
        request = controller
        onCleanup(() => controller.abort())
        saved.value = undefined
        issue.value = undefined
        saving.value = false
        loading.value = !!current && !!mediaRevision
        if (!current || !mediaRevision) return
        try {
            const response = await loadChapters(current, mediaRevision, controller.signal)
            if (controller.signal.aborted) return
            if (response) {
                saved.value = response
            } else if (current === previousPath && mediaRevision === previousRevision && attempt !== previousAttempt) {
                reloadMedia()
            } else {
                issue.value = 'chapterConflict'
            }
        } catch {
            if (!controller.signal.aborted) issue.value = 'chapterLoadFailed'
        } finally {
            if (!controller.signal.aborted) loading.value = false
        }
    }, {immediate: true})

    async function save(next: Chapter[]): Promise<boolean> {
        const controller = request
        if (!controller || !saved.value || saving.value || issue.value === 'chapterConflict') return false
        const currentPath = path.value
        const currentRevision = revision.value
        const current = () => !controller.signal.aborted && path.value === currentPath && revision.value === currentRevision
        saving.value = true
        issue.value = undefined
        try {
            const response = await saveChapters(currentPath, currentRevision, saved.value.version, next)
            if (!current()) return false
            if (!response) {
                issue.value = 'chapterConflict'
                return false
            }
            saved.value = response
            return true
        } catch (cause) {
            if (current()) {
                const reason = cause instanceof Error && cause.message
                issue.value = reason === 'chapterDurationUnavailable' || reason === 'chapterTimeInvalid'
                    ? reason : 'chapterSaveFailed'
            }
            return false
        } finally {
            if (current()) saving.value = false
        }
    }

    return {chapters, loading, saving, issue, ready: computed(() => !!saved.value), reload, save}
}

export interface ExplorerItem {
    name: string
    type: 'dir' | 'file' | 'other'
    size?: number
    modified?: number
}

async function post(path: string, body: object): Promise<Response> {
    const response = await fetch(path, {
        method: 'POST',
        headers: {'Content-Type': 'application/json'},
        body: JSON.stringify(body),
    })
    if (!response.ok) throw new Error(await response.text() || String(response.status))
    return response
}

export async function explore(path: string, includeOther: boolean): Promise<ExplorerItem[]> {
    return (await post('/api/explorer', {path, includeOther})).json()
}

export async function createDirectory(path: string): Promise<void> {
    await post('/api/mkdir', {path})
}

export function uploadFile(path: string, file: File, progress: (percent: number) => void): Promise<void> {
    return new Promise((resolve, reject) => {
        const form = new FormData()
        form.append('path', path)
        form.append('file', file)
        const request = new XMLHttpRequest()
        request.open('POST', '/api/upload')
        request.upload.onprogress = (event) => {
            if (event.lengthComputable) progress(Math.round(event.loaded / event.total * 100))
        }
        request.onload = () => {
            if (request.status >= 200 && request.status < 300) resolve()
            else reject(new Error(request.responseText || String(request.status)))
        }
        request.onerror = () => reject(new Error('Network error'))
        request.send(form)
    })
}

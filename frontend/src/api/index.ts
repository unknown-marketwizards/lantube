import axios from 'axios'


axios.defaults.withCredentials = true

export default {
    explorer: (path) => {
        let params = {'path': path}
        return axios.post('api/explorer', params).then(res => res.data)
    },
    mkdir: (path) => {
        let params = {'path': path}
        return axios.post('api/mkdir', params).then(res => res.data)
    },
}
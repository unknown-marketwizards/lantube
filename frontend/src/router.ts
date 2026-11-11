import {createRouter, createWebHashHistory} from 'vue-router'
import Home from './views/Home.vue'
import Explorer from './views/Explorer.vue'
import Setting from './views/Setting.vue'
import Videos from './views/Videos.vue'

export default createRouter({
    history: createWebHashHistory(),
    routes: [{
        path: '/',
        component: Home,
        redirect: {name: 'library'},
        children: [
            {path: 'explorer', name: 'library', component: Explorer},
            {path: 'videos', name: 'player', component: Videos},
            {path: 'setting', name: 'settings', component: Setting},
        ],
    }, {
        path: '/:pathMatch(.*)*',
        redirect: {name: 'library'},
    }],
    scrollBehavior(to, from, savedPosition) {
        if (savedPosition) return savedPosition
        if (to.name !== from.name || to.query.dir !== from.query.dir) return {top: 0}
    },
})

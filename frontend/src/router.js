import Vue from 'vue'
import Router from 'vue-router'
import Home from './views/Home.vue'
import Explorer from './views/Explorer.vue'
import Setting from './views/Setting.vue'
import Videos from './views/Videos.vue'

Vue.use(Router)

export default new Router({
    routes: [
        {
            path: '/',
            name: 'explorer',
            component: Home,
            redirect: '/explorer',
            leaf: true,
            menuShow: true,
            iconCls: 'el-icon-menu',
            children: [
                {path: '/explorer', component: Explorer, name: 'router.explorer', title: 'router.explorer', menuShow: true}
            ]
        },
        {
            path: '/',
            name: 'setting',
            component: Home,
            redirect: '/setting',
            leaf: true,
            menuShow: true,
            iconCls: 'el-icon-setting',
            children: [
                {path: '/setting', component: Setting, name: 'router.setting', title: 'router.setting', menuShow: true}
            ]
        },
        {
            path: '/',
            name: 'videos',
            component: Home,
            leaf: false,
            menuShow: false,
            children: [
                {path: '/videos', component: Videos, name: 'router.video', title: 'router.video', menuShow: true}
            ]
        },
    ]
})

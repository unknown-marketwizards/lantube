<template>
    <el-container>
        <el-aside width="80px">
            <el-menu :default-active="defaultActiveIndex" router :collapse="true" @select="handleSelect">
                <template v-for="(item,index) in $router.options.routes">
                    <el-submenu v-if="!item.leaf && item.menuShow" :index="index+''" :key="index+'0'">
                        <template slot="title">
                            <i :class="item.iconCls"></i>
                            <span slot="title">{{ $t(item.name) }}</span>
                        </template>
                        <template v-for="term in item.children">
                            <el-menu-item v-if="term.menuShow" :key="term.path" :index="term.path"
                                          :class="$route.path===term.path?'is-active':''">
                                <i :class="term.iconCls"></i><span slot="title">{{ $t(term.name) }}</span>
                            </el-menu-item>
                        </template>
                    </el-submenu>
                    <el-menu-item v-else-if="item.leaf && item.children && item.children.length"
                                  :index="item.children[0].path"
                                  :class="$route.path===item.children[0].path?'is-active':''" :key="index+'1'">
                        <i :class="item.iconCls"></i>
                        <span slot="title">{{ $t(item.children[0].name) }}</span>
                    </el-menu-item>
                </template>
            </el-menu>
        </el-aside>

        <el-main>
            <router-view></router-view>
        </el-main>

    </el-container>
</template>

<script>

export default {
    data() {
        return {
            defaultActiveIndex: "0",
        };
    },
    methods: {
        handleSelect(index) {
            this.defaultActiveIndex = index;
        },
    }
}
</script>


<style scoped>

.container {
    position: absolute;
    top: 0px;
    bottom: 0px;
    width: 100%;
}

.main {
    display: flex;
}

</style>

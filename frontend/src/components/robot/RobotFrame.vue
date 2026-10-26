<script setup>
import { ref } from 'vue';
import { useRoute } from 'vue-router';
import { useI18n } from 'vue-i18n'
import MaterialSymbolsHouseOutline from '~icons/material-symbols/house-outline'
import RiRobot2Line from '~icons/ri/robot-2-line'
import BiChatSquareDots from '~icons/bi/chat-square-dots'
import MaterialSymbolsBook5Outline from '~icons/material-symbols/book-5-outline'
import RiBardLine from '~icons/ri/bard-line'
import SolarDownloadOutline from '~icons/solar/download-outline'
import SolarRouting2Linear from '~icons/solar/routing-2-linear'
import EpSetting from '~icons/ep/setting'
import EpDArrowLeft from '~icons/ep/d-arrow-left'
import EpDArrowRight from '~icons/ep/d-arrow-right'
const route = useRoute()
const { t, locale } = useI18n();
const robotId = route.params.robotId
const isCollapse = ref(false)
</script>
<style scoped>
.frame {
    min-height: 100vh;
    background: #f6f8fb;
}

.sidebar {
    display: flex;
    flex-direction: column;
    height: 100vh;
    position: sticky;
    top: 0;
    background: linear-gradient(180deg, #1e2532 0%, #232b3b 100%);
    transition: width 0.25s ease;
    overflow: hidden;
}

.sidebar-logo {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 20px 18px;
    color: #fff;
    font-weight: 700;
    font-size: 15px;
    white-space: nowrap;
}

.logo-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    border-radius: 10px;
    background: linear-gradient(135deg, #6366f1, #8b5cf6);
    color: #fff;
    font-size: 18px;
    flex-shrink: 0;
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.4);
}

.sidebar-menu {
    border-right: none;
    background: transparent;
    flex: 1;
}

.sidebar-menu :deep(.el-menu-item),
.sidebar-menu :deep(.el-sub-menu__title) {
    color: #9aa5b8;
    margin: 4px 10px;
    border-radius: 10px;
    height: 44px;
    line-height: 44px;
    transition: all 0.2s ease;
}

.sidebar-menu :deep(.el-menu-item:hover),
.sidebar-menu :deep(.el-sub-menu__title:hover) {
    color: #fff;
    background: rgba(255, 255, 255, 0.08);
}

.sidebar-menu :deep(.el-menu-item.is-active) {
    color: #fff;
    background: linear-gradient(135deg, #6366f1, #8b5cf6);
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.4);
}

.sidebar-menu :deep(.el-menu-item .el-icon),
.sidebar-menu :deep(.el-sub-menu__title .el-icon) {
    color: inherit;
}

.sidebar-menu :deep(.el-menu) {
    background: transparent;
}

.sidebar-menu :deep(.el-menu .el-menu-item) {
    background: rgba(255, 255, 255, 0.03);
}

.collapse-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    margin: 12px 10px 16px;
    padding: 8px 0;
    border-radius: 10px;
    color: #9aa5b8;
    font-size: 16px;
    cursor: pointer;
    background: rgba(255, 255, 255, 0.05);
    transition: all 0.2s ease;
    user-select: none;
    white-space: nowrap;
    overflow: hidden;
}

.collapse-btn:hover {
    color: #fff;
    background: rgba(255, 255, 255, 0.1);
}

.collapse-label {
    font-size: 12px;
    letter-spacing: 0.05em;
}

.frame-main {
    padding: 24px 32px;
    min-width: 0;
    /* el-main defaults to overflow: auto, which becomes the sticky
       positioning viewport; since the actual scrolling happens on the
       window, that breaks position: sticky inside child pages. */
    overflow: visible;
}
</style>
<template>
    <el-container class="frame">
        <el-aside :width="isCollapse ? '64px' : '216px'" class="sidebar">
            <div class="sidebar-logo">
                <span class="logo-icon">
                    <RiRobot2Line />
                </span>
                <span v-show="!isCollapse">{{ t('menu.thisRobot') }}</span>
            </div>
            <el-menu class="sidebar-menu" :collapse="isCollapse" :collapse-transition="false" router
                :default-active="route.path">
                <el-menu-item index="/">
                    <el-icon>
                        <MaterialSymbolsHouseOutline />
                    </el-icon>
                    <template #title>{{ t('menu.home') }}</template>
                </el-menu-item>
                <el-menu-item :index="'/robot/' + robotId">
                    <el-icon>
                        <RiRobot2Line />
                    </el-icon>
                    <template #title>{{ t('menu.thisRobot') }}</template>
                </el-menu-item>
                <el-menu-item :index="'/robot/' + robotId + '/mainflows'">
                    <el-icon>
                        <BiChatSquareDots />
                    </el-icon>
                    <template #title>{{ t('menu.dialogFlows') }}</template>
                </el-menu-item>
                <el-sub-menu index="kbMenu">
                    <template #title>
                        <el-icon>
                            <MaterialSymbolsBook5Outline />
                        </el-icon>
                        <span>{{ t('menu.kb') }}</span>
                    </template>
                    <el-menu-item :index="'/robot/' + robotId + '/kb/qa'">{{ t('menu.qa') }}</el-menu-item>
                    <el-menu-item :index="'/robot/' + robotId + '/kb/doc'">{{ t('menu.doc') }}</el-menu-item>
                </el-sub-menu>
                <el-menu-item :index="'/robot/' + robotId + '/intents'">
                    <el-icon>
                        <RiBardLine />
                    </el-icon>
                    <template #title>{{ t('menu.intents') }}</template>
                </el-menu-item>
                <el-menu-item :index="'/robot/' + robotId + '/variables'">
                    <el-icon>
                        <SolarDownloadOutline />
                    </el-icon>
                    <template #title>{{ t('menu.vars') }}</template>
                </el-menu-item>
                <el-menu-item :index="'/robot/' + robotId + '/external/httpApis'">
                    <el-icon>
                        <SolarRouting2Linear />
                    </el-icon>
                    <template #title>{{ t('menu.eApi') }}</template>
                </el-menu-item>
                <el-menu-item :index="'/robot/' + robotId + '/settings'">
                    <el-icon>
                        <EpSetting />
                    </el-icon>
                    <template #title>{{ t('menu.rs') }}</template>
                </el-menu-item>
            </el-menu>
            <div class="collapse-btn" @click="isCollapse = !isCollapse">
                <el-icon>
                    <EpDArrowRight v-if="isCollapse" />
                    <EpDArrowLeft v-else />
                </el-icon>
                <span v-show="!isCollapse" class="collapse-label">Collapse</span>
            </div>
        </el-aside>
        <el-main class="frame-main"><router-view></router-view></el-main>
    </el-container>
</template>

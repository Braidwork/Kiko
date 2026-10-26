<template>
    <div style="text-align: right;">
        <div class="language-switcher">
            {{ locale === 'zh' ? 'Language' : '语言切换' }}
            <el-dropdown v-model="isOpen" @command="switchLanguage">
                <span class="el-dropdown-link lang-button">
                    <img :src="currentFlag" alt="flag" class="flag-icon" />
                    {{ currentLangLabel }}
                    <i class="el-icon-arrow-down el-icon--right"></i>
                </span>
                <template #dropdown>
                    <el-dropdown-menu>
                        <el-dropdown-item v-for="lang in languages" :key="lang.code" :command="lang.code"
                            class="dropdown-item">
                            <img :src="lang.flag" alt="" class="flag-icon" />
                            {{ lang.label }}
                        </el-dropdown-item>
                    </el-dropdown-menu>
                </template>
            </el-dropdown>
        </div>
    </div>
</template>

<script setup>
import { ref, computed } from 'vue'
import { useI18n } from 'vue-i18n'
// https://www.jsdelivr.com/package/npm/flag-icons-svg
import zhFlag from '../assets/flags/cn.svg'
import ukFlag from '../assets/flags/gb.svg'
const { locale } = useI18n()

// 模拟 i18n 切换逻辑
const supportedLanguages = {
    en: {
        code: 'en',
        label: 'English',
        flag: ukFlag
    },
    zh: {
        code: 'zh',
        label: '简体中文',
        flag: zhFlag
    }
}

const languages = Object.values(supportedLanguages)

const selectedLang = ref(locale.value)

const isOpen = ref(false)

const currentLangLabel = computed(() => {
    return supportedLanguages[selectedLang.value]?.label || 'Language'
})

const currentFlag = computed(() => {
    return supportedLanguages[selectedLang.value]?.flag
})

function switchLanguage(lang) {
    if (lang === locale.value) return
    locale.value = lang
    localStorage.setItem('lang', lang)
    selectedLang.value = lang
    isOpen.value = false
    console.log('切换语言至:', lang)
}
</script>

<style scoped>
.language-switcher {
  display: inline-block;
}

.lang-button {
  display: flex;
  align-items: center;
  padding: 8px 12px;
  border-radius: 4px;
  cursor: pointer;
  font-size: 14px;
  color: #333;
}

.lang-button:hover {
  background-color: #f5f7fa;
}

.flag-icon {
  width: 20px;
  height: auto;
  margin-right: 6px;
}

.dropdown-item {
  display: flex;
  align-items: center;
}

.dropdown-item img {
  width: 18px;
  margin-right: 8px;
}
</style>
<script setup>
import { nextTick, reactive, onMounted, ref, computed } from 'vue';
import { useRoute, useRouter } from 'vue-router';
// import { ElMessage, ElMessageBox } from 'element-plus'
import { httpReq } from '../../assets/tools.js'
import { useI18n } from 'vue-i18n'
const { t, tm, rt } = useI18n();
import RiBardLine from '~icons/ri/bard-line';
import RiArrowLeftLine from '~icons/ri/arrow-left-line';
import RiKey2Line from '~icons/ri/key-2-line';
import RiCodeSSlashLine from '~icons/ri/code-s-slash-line';
import RiChatQuoteLine from '~icons/ri/chat-quote-line';
import RiSparkling2Line from '~icons/ri/sparkling-2-line';
import RiRefreshLine from '~icons/ri/refresh-line';
import RiAddLine from '~icons/ri/add-line';
import EpPlus from '~icons/ep/plus';
const route = useRoute();
const router = useRouter();
const robotId = route.params.robotId;
const intentName = ref('')

const intentData = reactive({
    keywords: [],
    regexes: [],
    phrases: [],
});

const formData = {
    robotId: '',
    id: '',
    data: '',
};

onMounted(async () => {
    formData.robotId = robotId;
    formData.id = route.query.id;
    let t = await httpReq('GET', 'intent/detail', formData, null, null);
    console.log(t.data);
    if (t.status == 200 && t.data) {
        intentName.value = t.data.intent_name;
        intentData.keywords = t.data.keywords;
        intentData.regexes = t.data.regexes;
        intentData.phrases = t.data.phrases.map((cur, idx, arr) => cur.phrase);
    }
    t = await httpReq("GET", 'management/settings/model/check/embedding', { robotId: robotId }, null, null);
    // console.log(t);
    phraseInputDisabled.value = t == null || t.status == null || t.status != 200;
    // console.log(phraseInputDisabled.value)
});

//keyword
const keywordValue = ref('');
const keywordInputVisible = ref(false);
const keywordInputRef = ref();
const showKeyWordInput = () => {
    keywordInputVisible.value = true
    nextTick(() => {
        keywordInputRef.value.focus()
    })
}

async function newKeyword() {
    if (keywordValue.value) {
        formData.id = route.query.id;
        formData.data = keywordValue.value;
        const t = await httpReq('POST', 'intent/keyword', { id: formData.id, data: route.query.idx }, null, formData);
        console.log(t.data);
        if (t.status == 200)
            intentData.keywords.push(keywordValue.value)
    }
    keywordInputVisible.value = false
    keywordValue.value = ''
}

async function removeKeyword(w) {
    ElMessageBox.confirm(
        w + ' will be deleted permanently. Continue?',
        'Warning',
        {
            confirmButtonText: 'OK',
            cancelButtonText: 'Cancel',
            type: 'warning',
        }
    )
        .then(async () => {
            const idx = intentData.keywords.indexOf(w);
            formData.id = route.query.id;
            formData.data = idx.toString();
            const t = await httpReq('DELETE', 'intent/keyword', null, null, formData);
            console.log(t.data);
            if (t.status == 200) {
                intentData.keywords.splice(idx, 1);
                ElMessage({
                    type: 'success',
                    message: 'Delete completed',
                })
            }
        })
        .catch(() => {
            // ElMessage({
            //     type: 'info',
            //     message: 'Delete canceled',
            // })
        })
}

//regex
const regexValue = ref('');
const regexInputVisible = ref(false);
const regexInputRef = ref();
const showRegexInput = () => {
    regexInputVisible.value = true
    nextTick(() => {
        regexInputRef.value.focus()
    })
}

async function newRegex() {
    if (regexValue.value) {
        formData.id = route.query.id;
        formData.data = regexValue.value;
        const t = await httpReq('POST', 'intent/regex', { id: formData.id, data: route.query.idx }, null, formData);
        console.log(t.data);
        if (t.status == 200)
            intentData.regexes.push(regexValue.value)
    }
    regexInputVisible.value = false
    regexValue.value = ''
}

async function removeRegex(w) {
    ElMessageBox.confirm(
        w + ' will be deleted permanently. Continue?',
        'Warning',
        {
            confirmButtonText: 'OK',
            cancelButtonText: 'Cancel',
            type: 'warning',
        }
    )
        .then(async () => {
            const idx = intentData.regexes.indexOf(w);
            formData.id = route.query.id;
            formData.data = idx.toString();
            const t = await httpReq('DELETE', 'intent/regex', null, null, formData);
            console.log(t.data);
            if (t.status == 200) {
                intentData.regexes.splice(idx, 1);
                ElMessage({
                    type: 'success',
                    message: 'Delete completed',
                })
            }
        })
        .catch(() => {
            // ElMessage({
            //     type: 'info',
            //     message: 'Delete canceled',
            // })
        })
}

//phrase
const phraseValue = ref('');
const phraseInputDisabled = ref(true);
const phraseInputVisible = ref(false);
const phraseInputRef = ref();
const addPhraseFailedAlertTitle = ref('')
const showAddedPhraseFailedTip = ref(false)
const regeneratingAllEmbeddings = ref(false)
const showPhraseInput = () => {
    phraseInputVisible.value = true
    nextTick(() => {
        phraseInputRef.value.focus()
    })
}

async function newPhrase() {
    if (phraseValue.value) {
        formData.id = route.query.id;
        formData.data = phraseValue.value;
        const t = await httpReq('POST', 'intent/phrase', { robotId: robotId, id: formData.id, data: route.query.idx }, null, formData);
        // console.log(t.data);
        if (t.status == 200)
            intentData.phrases.push(phraseValue.value)
        else {
            addPhraseFailedAlertTitle.value = 'Added similar sentence failed: ' + t.err.message;
            // ElMessage.error(t.err.message);
            showAddedPhraseFailedTip.value = true
        }
    }
    phraseInputVisible.value = false
    phraseValue.value = ''
}

async function removePhrase(w) {
    ElMessageBox.confirm(
        w + ' will be deleted permanently. Continue?',
        'Warning',
        {
            confirmButtonText: 'OK',
            cancelButtonText: 'Cancel',
            type: 'warning',
        }
    )
        .then(async () => {
            const idx = intentData.phrases.indexOf(w);
            formData.id = route.query.id;
            formData.data = idx.toString();
            const t = await httpReq('DELETE', 'intent/phrase', null, null, formData);
            console.log(t.data);
            if (t.status == 200) {
                intentData.phrases.splice(idx, 1);
                ElMessage({
                    type: 'success',
                    message: 'Delete completed',
                })
            }
        })
        .catch(() => {
            // ElMessage({
            //     type: 'info',
            //     message: 'Delete canceled',
            // })
        })
}

const regenerateAll = async () => {
    const t = await httpReq("GET", 'management/settings', { robotId: robotId }, null, null)
    console.log(t);
    if (t.status == 200 && t.data) {
        if (t.data.sentenceEmbeddingProvider.provider.id == 'OpenAICompatible') {
            ElMessageBox.confirm(
                'The sentence embedding providor is a remote (paid) one, this will incur some fees. Continue?',
                'Warning',
                {
                    confirmButtonText: 'Regenerate all',
                    cancelButtonText: 'Cancel',
                    type: 'warning',
                }
            )
                .then(async () => {
                    doRegenerateAll()
                })
                .catch(() => {
                })
            return;
        }
    }
    doRegenerateAll()
}

const doRegenerateAll = async () => {
    regeneratingAllEmbeddings.value = true
    httpReq('GET', 'intent/phrase/regenerate-all', { robotId: robotId, id: formData.id, data: '' }, null, null).then(v => regeneratingAllEmbeddings.value = false);
}

const goBack = () => {
    router.push({ name: 'intents', params: { robotId: robotId } })
}
</script>
<style scoped>
.detail-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 12px;
    margin-bottom: 20px;
}

.detail-header-left {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
}

.detail-title {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 22px;
    font-weight: 700;
    color: var(--app-text);
}

.detail-title .detail-title-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 38px;
    height: 38px;
    border-radius: 10px;
    background: #eef2ff;
    color: var(--app-primary);
    font-size: 20px;
    flex-shrink: 0;
}

.detail-subtitle {
    margin-top: 2px;
    font-size: 13px;
    color: var(--app-text-secondary);
}

.tag-groups {
    display: flex;
    flex-direction: column;
    gap: 18px;
}

.tag-group {
    background: #fff;
    border: 1px solid var(--app-card-border);
    border-radius: 14px;
    padding: 20px 22px;
    box-shadow: 0 1px 3px rgba(31, 45, 61, 0.04);
    transition: box-shadow 0.2s;
}

.tag-group:hover {
    box-shadow: 0 4px 14px rgba(31, 45, 61, 0.07);
}

.tag-group-head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 16px;
    font-size: 16px;
    font-weight: 600;
    color: #1f2d3d;
}

.tag-group-head .tag-group-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    border-radius: 10px;
    font-size: 17px;
    flex-shrink: 0;
}

.tag-group-icon.indigo {
    color: #6366f1;
    background: #eef2ff;
}

.tag-group-icon.amber {
    color: #d97706;
    background: #fef3c7;
}

.tag-group-icon.emerald {
    color: #059669;
    background: #d1fae5;
}

.tag-group-hint {
    font-size: 12px;
    font-weight: 400;
    color: #a0a6b1;
}

.count-badge {
    display: inline-block;
    min-width: 26px;
    padding: 1px 9px;
    border-radius: 999px;
    text-align: center;
    font-size: 12px;
    font-weight: 600;
    background: #f3f4f6;
    color: #6b7280;
}

.tag-group-body {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    min-height: 34px;
}

/* Colorful tags per section */
.tag-group-body .el-tag {
    max-width: 100%;
    border-radius: 8px;
    font-weight: 500;
    padding: 0 10px;
    border: none;
}

.tag-group.kw .el-tag {
    background: #eef2ff;
    color: #4f46e5;
}

.tag-group.re .el-tag {
    background: #fef3c7;
    color: #b45309;
}

.tag-group.sp .el-tag {
    background: #d1fae5;
    color: #047857;
}

.tag-group-body .el-tag :deep(.el-tag__close) {
    color: inherit;
    opacity: 0.55;
}

.tag-group-body .el-tag :deep(.el-tag__close:hover) {
    background: rgba(0, 0, 0, 0.06);
    color: inherit;
    opacity: 1;
}

.tag-group-body .el-input {
    width: 160px;
}

/* Mono style for regex tags */
.tag-group.re .el-tag {
    font-family: 'JetBrains Mono', 'Fira Code', Consolas, monospace;
    font-size: 12.5px;
}

.tag-group-empty {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: #b3b9c4;
    padding: 4px 2px;
}

.disabled-tip {
    margin-top: 14px;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 10px 14px;
    border-radius: 10px;
    background: #f9fafb;
    border: 1px dashed #e5e7eb;
    font-size: 13px;
    color: #86909c;
    line-height: 1.8;
}

.footer-actions {
    margin-top: 20px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 12px;
}
</style>
<template>
    <div class="detail-header">
        <div class="detail-header-left">
            <el-button text @click="goBack">
                <el-icon style="margin-right: 4px"><RiArrowLeftLine /></el-icon>
                {{ t('common.back') }}
            </el-button>
        </div>
    </div>

    <div class="detail-header" style="margin-bottom: 16px;">
        <div class="detail-header-left">
            <h1 class="detail-title">
                <span class="detail-title-icon"><RiBardLine /></span>
                {{ $t('intent.detail.edit') }}: {{ intentName }}
            </h1>
        </div>
        <div class="detail-subtitle">{{ intentData.keywords.length + intentData.regexes.length + intentData.phrases.length }} entries total</div>
    </div>

    <div class="tag-groups">
        <!-- Keywords -->
        <div class="tag-group kw">
            <div class="tag-group-head">
                <span class="tag-group-icon indigo"><RiKey2Line /></span>
                {{ $t('intent.detail.kw') }}
                <span class="count-badge">{{ intentData.keywords.length }}</span>
                <span class="tag-group-hint">Case insensitive</span>
            </div>
            <div class="tag-group-body">
                <el-tag v-for="tag in intentData.keywords" :key="tag" closable
                    :disable-transitions="false" @close="removeKeyword(tag)">
                    {{ tag }}
                </el-tag>
                <span class="tag-group-empty" v-if="!keywordInputVisible && intentData.keywords.length === 0">
                    No keywords yet —
                </span>
                <el-input v-if="keywordInputVisible" ref="keywordInputRef" v-model="keywordValue" size="small"
                    placeholder="Press Enter to add" @keyup.enter="newKeyword" @blur="newKeyword" />
                <el-button v-else size="small" plain class="add-btn" @click="showKeyWordInput">
                    <el-icon style="margin-right: 4px"><EpPlus /></el-icon>
                    {{ $t('intent.detail.addKw') }}
                </el-button>
            </div>
        </div>

        <!-- Regexes -->
        <div class="tag-group re">
            <div class="tag-group-head">
                <span class="tag-group-icon amber"><RiCodeSSlashLine /></span>
                {{ $t('intent.detail.re') }}
                <span class="count-badge">{{ intentData.regexes.length }}</span>
            </div>
            <div class="tag-group-body">
                <el-tag v-for="tag in intentData.regexes" :key="tag" closable
                    :disable-transitions="false" @close="removeRegex(tag)">
                    {{ tag }}
                </el-tag>
                <span class="tag-group-empty" v-if="!regexInputVisible && intentData.regexes.length === 0">
                    No regexes yet —
                </span>
                <el-input v-if="regexInputVisible" ref="regexInputRef" v-model="regexValue" size="small"
                    placeholder="Press Enter to add" @keyup.enter="newRegex" @blur="newRegex" />
                <el-button v-else size="small" plain class="add-btn" @click="showRegexInput">
                    <el-icon style="margin-right: 4px"><EpPlus /></el-icon>
                    {{ $t('intent.detail.addRe') }}
                </el-button>
            </div>
        </div>

        <!-- Similar phrases -->
        <div class="tag-group sp">
            <div class="tag-group-head">
                <span class="tag-group-icon emerald"><RiChatQuoteLine /></span>
                {{ $t('intent.detail.sp') }}
                <span class="count-badge">{{ intentData.phrases.length }}</span>
            </div>
            <div class="tag-group-body">
                <el-tag v-for="tag in intentData.phrases" :key="tag" closable
                    :disable-transitions="false" @close="removePhrase(tag)">
                    {{ tag }}
                </el-tag>
                <span class="tag-group-empty" v-if="!phraseInputVisible && intentData.phrases.length === 0 && !phraseInputDisabled">
                    No similar phrases yet —
                </span>
                <el-input v-if="phraseInputVisible" ref="phraseInputRef" v-model="phraseValue" size="small"
                    placeholder="Press Enter to add" @keyup.enter="newPhrase" />
                <el-button v-else size="small" plain class="add-btn" @click="showPhraseInput" :disabled="phraseInputDisabled">
                    <el-icon style="margin-right: 4px"><EpPlus /></el-icon>
                    {{ $t('intent.detail.addSp') }}
                </el-button>
            </div>
            <div class="disabled-tip" v-show="phraseInputDisabled">
                <el-icon style="margin-top: 4px; flex-shrink: 0"><RiSparkling2Line /></el-icon>
                <span>
                    This feature was disabled because <b>local model files were missing</b> or <b>the sentence
                        embedding API key is empty</b>, please
                    goto <router-link :to="{ name: 'settings', params: { robotId: robotId } }">settings</router-link> and
                    select one
                    model first.
                </span>
            </div>
        </div>
    </div>

    <div class="footer-actions">
        <el-alert v-if="showAddedPhraseFailedTip" :title="addPhraseFailedAlertTitle" type="error"
            description="But don't worry, maybe you switched different embedding provider caused this. You can press 'Regenerate all similar sentences.' button below to fix this issue."
            show-icon style="flex: 1; min-width: 280px;" />
        <el-button v-show="!phraseInputDisabled" type="warning" plain :loading="regeneratingAllEmbeddings"
            @click="regenerateAll">
            <el-icon v-if="!regeneratingAllEmbeddings" style="margin-right: 6px"><RiRefreshLine /></el-icon>
            Regenerate all similar sentences.
        </el-button>
    </div>
</template>

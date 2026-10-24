<script setup>
import { computed, nextTick, onMounted, reactive, ref } from 'vue';
import { useRoute } from 'vue-router';
import { httpReq, cloneObj } from '../../assets/tools.js'
import { useI18n } from 'vue-i18n'
const { t, tm } = useI18n();
import MaterialSymbolsBook5Outline from '~icons/material-symbols/book-5-outline';
import EpPlus from '~icons/ep/plus';
import EpMinus from '~icons/ep/minus';
const route = useRoute();
const robotId = route.params.robotId
const qaData = reactive({
    id: null,
    question: {
        question: ''
    },
    similarQuestions: [],
    answer: '',
})
const tableData = reactive([])
const listQa = async () => {
    const resp = await httpReq('GET', 'kb/qa', { robotId: robotId }, null, null);
    if (resp.status == 200)
        tableData.splice(0, tableData.length, ...resp.data);
}
onMounted(() => {
    listQa();
})
// Deep-clone the row into the form, so editing never mutates tableData directly
// (also preserves question/similarQuestions vec_row_id without stale leftovers)
const resetForm = (d) => {
    qaData.id = d ? d.id : null;
    qaData.question = d ? cloneObj(d.question) : { question: '' };
    qaData.similarQuestions = d ? cloneObj(d.similarQuestions) : [];
    qaData.answer = d ? d.answer : '';
}
const newQa = () => {
    resetForm(null);
    dialogVisible.value = true
}
const showQaDetail = (idx) => {
    const d = tableData[idx];
    if (d) {
        qaDetailIdx.value = idx
        resetForm(d)
        qaDetailVisible.value = true
    }
}
const editQa = (idx) => {
    const d = tableData[idx];
    if (d) {
        resetForm(d)
        dialogVisible.value = true
    }
}
const saving = ref(false)
const saveQa = async () => {
    if (!formRef.value || !(await formRef.value.validate().catch(() => false)))
        return;
    // Trim and drop blank similar questions before submitting
    const payload = cloneObj(qaData);
    payload.question.question = payload.question.question.trim();
    payload.similarQuestions = payload.similarQuestions
        .map(q => ({ ...q, question: (q.question || '').trim() }))
        .filter(q => q.question !== '');
    payload.answer = payload.answer.trim();
    saving.value = true;
    try {
        const resp = await httpReq('POST', 'kb/qa', { robotId: robotId }, null, payload);
        if (resp.status == 200) {
            ElMessage.success(t('common.saved'));
            dialogVisible.value = false
            listQa()
        } else {
            ElMessage.error(resp.err?.message || t('common.errTip'));
        }
    } finally {
        saving.value = false;
    }
}
const deleteQa = async (idx) => {
    const d = tableData[idx];
    if (!d)
        return;
    ElMessageBox.confirm(
        t('kb.qa.delConfirm'),
        'Warning',
        {
            confirmButtonText: t('common.del'),
            cancelButtonText: t('common.cancel'),
            type: 'warning',
        }
    ).then(async () => {
        const payload = {
            id: d.id,
            question: d.question,
            similarQuestions: d.similarQuestions,
            answer: d.answer,
        };
        const resp = await httpReq('DELETE', 'kb/qa', { robotId: robotId }, null, payload);
        if (resp.status == 200) {
            ElMessage.success(t('common.deleted'));
            nextTick(() => {
                qaDetailVisible.value = false
                listQa()
            })
        } else {
            ElMessage.error(resp.err?.message || t('common.errTip'));
        }
    }).catch(() => {
        // Delete canceled
    })
}
const loading = ref(false)
const testQnAText = ref('')
const testQnAResult = ref('')
const testQa = async () => {
    if (!testQnAText.value.trim())
        return;
    loading.value = true;
    try {
        const resp = await httpReq('GET', 'kb/qa/dryrun', { robotId: robotId, text: testQnAText.value }, null, null);
        if (resp.status == 200)
            testQnAResult.value = resp.data[0].answer + ' (Distance: ' + resp.data[1] + ')';
        else
            testQnAResult.value = resp.err?.message || t('common.errTip');
    } finally {
        loading.value = false;
    }
}

const dialogVisible = ref(false)
const dialogTitle = computed(() => qaData.id == null ? t('kb.qa.addTitle') : t('kb.qa.editTitle'))
const qaDetailVisible = ref(false)
const qaDetailIdx = ref(0)
const dryRunFormVisible = ref(false)
const formRef = ref()
const rules = {
    'question.question': [{ required: true, message: () => t('kb.qa.rules.question'), trigger: 'blur' }],
    answer: [{ required: true, message: () => t('kb.qa.rules.answer'), trigger: 'blur' }],
}
const formLabelWidth = '120px'
</script>
<style scoped>
.similar-count {
    display: inline-block;
    min-width: 28px;
    padding: 2px 10px;
    border-radius: 999px;
    text-align: center;
    font-size: 13px;
    font-weight: 600;
    background: #eef2ff;
    color: #6366f1;
}

.qa-question {
    font-weight: 600;
    color: #1f2d3d;
}

.qa-answer {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    color: #4e5969;
}

.similar-row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
}

.similar-row .el-input {
    flex: 1;
}

.similar-add {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
}

.form-tip {
    font-size: 12px;
    color: #98a2b3;
    line-height: 1.5;
}

/* ---- QA detail drawer ---- */
.qa-detail {
    display: flex;
    flex-direction: column;
    gap: 20px;
    padding-bottom: 16px;
}

.qa-detail__section {
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.qa-detail__label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.3px;
    color: #98a2b3;
}

.qa-detail__card {
    padding: 12px 14px;
    border: 1px solid #eef0f4;
    border-radius: 10px;
    background: #f7f8fa;
    font-size: 14px;
    line-height: 1.7;
    color: #1f2d3d;
    word-break: break-word;
}

.qa-detail__card--primary {
    border-color: #dfe4ff;
    border-left: 3px solid #6366f1;
    background: #eef2ff;
    font-weight: 600;
}

/* Answer may contain newlines — keep the author's line breaks */
.qa-detail__card--answer {
    min-height: 72px;
    white-space: pre-wrap;
    color: #4e5969;
}

.qa-detail__similar {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0;
    padding: 0;
    list-style: none;
}

.qa-detail__similar-item {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid #eef0f4;
    border-radius: 10px;
    background: #f7f8fa;
    font-size: 14px;
    line-height: 1.6;
    color: #4e5969;
    transition: background 0.2s, border-color 0.2s;
}

.qa-detail__similar-item:hover {
    border-color: #dfe4ff;
    background: #eef2ff;
}

.qa-detail__similar-idx {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    margin-top: 1px;
    border: 1px solid #dfe4ff;
    border-radius: 50%;
    background: #fff;
    font-size: 12px;
    font-weight: 600;
    color: #6366f1;
}

.qa-detail__similar-text {
    flex: 1;
    word-break: break-word;
}
</style>
<template>
    <div class="page-header">
        <h1 class="page-title">
            <span class="page-title-icon"><MaterialSymbolsBook5Outline /></span>
            {{ $t('kb.qa.title') }}
        </h1>
        <div class="page-actions">
            <el-button @click="dryRunFormVisible = true">{{ $t('kb.qa.test') }}</el-button>
            <el-button type="primary" @click="newQa">
                <el-icon style="margin-right: 6px"><EpPlus /></el-icon>
                {{ $t('kb.qa.add') }}
            </el-button>
        </div>
    </div>
    <div class="page-card">
        <el-table :data="tableData" stripe style="width: 100%" :empty-text="t('kb.qa.empty')">
            <el-table-column prop="question.question" :label="tm('kb.qa.table')[0]" min-width="300">
                <template #default="scope">
                    <span class="qa-question">{{ scope.row.question.question }}</span>
                </template>
            </el-table-column>
            <el-table-column :label="tm('kb.qa.table')[1]" width="190" align="center">
                <template #default="scope">
                    <span class="similar-count">{{ scope.row.similarQuestions.length }}</span>
                </template>
            </el-table-column>
            <el-table-column prop="answer" :label="tm('kb.qa.table')[2]" min-width="240">
                <template #default="scope">
                    <span class="qa-answer">{{ scope.row.answer }}</span>
                </template>
            </el-table-column>
            <el-table-column fixed="right" :label="tm('kb.qa.table')[3]" width="200" align="center">
                <template #default="scope">
                    <el-button link type="primary" @click="showQaDetail(scope.$index)">{{ $t('common.toDetail') }}
                    </el-button>
                    <el-button link type="primary" @click="editQa(scope.$index)">{{ $t('common.edit') }}</el-button>
                    <el-button link type="danger" @click="deleteQa(scope.$index)">{{ $t('common.del') }}</el-button>
                </template>
            </el-table-column>
        </el-table>
    </div>
    <el-dialog v-model="dialogVisible" :title="dialogTitle" width="70%" destroy-on-close>
        <el-form ref="formRef" :model="qaData" :rules="rules">
            <el-form-item :label="$t('kb.qa.form.question')" prop="question.question" :label-width="formLabelWidth">
                <el-input v-model="qaData.question.question" :placeholder="$t('kb.qa.form.questionPH')" maxlength="200" />
            </el-form-item>
            <el-form-item v-for="(item, index) in qaData.similarQuestions" :key="index"
                :label="index == 0 ? $t('kb.qa.form.similar') : ''" :label-width="formLabelWidth">
                <div class="similar-row">
                    <el-input v-model="item.question" :placeholder="$t('kb.qa.form.similarPH')" maxlength="200" />
                    <el-button circle type="danger" plain @click="qaData.similarQuestions.splice(index, 1)">
                        <el-icon><EpMinus /></el-icon>
                    </el-button>
                </div>
            </el-form-item>
            <el-form-item label="" :label-width="formLabelWidth">
                <div class="similar-add">
                    <el-button plain @click="qaData.similarQuestions.push({ question: '' })">
                        <el-icon style="margin-right: 4px"><EpPlus /></el-icon>
                        {{ $t('kb.qa.form.addSimilar') }}
                    </el-button>
                    <span class="form-tip">{{ $t('kb.qa.form.similarTip') }}</span>
                </div>
            </el-form-item>
            <el-form-item :label="$t('kb.qa.form.answer')" prop="answer" :label-width="formLabelWidth">
                <el-input v-model="qaData.answer" :placeholder="$t('kb.qa.form.answerPH')" type="textarea" :rows="5"
                    maxlength="2000" show-word-limit />
            </el-form-item>
        </el-form>
        <template #footer>
            <div class="dialog-footer">
                <el-button @click="dialogVisible = false">{{ $t('common.cancel') }}</el-button>
                <el-button type="primary" :loading="saving" @click="saveQa">
                    {{ $t('common.save') }}
                </el-button>
            </div>
        </template>
    </el-dialog>
    <el-drawer v-model="qaDetailVisible" :title="$t('kb.qa.detail')" direction="rtl" size="60%">
        <div class="qa-detail">
            <section class="qa-detail__section">
                <div class="qa-detail__label">{{ $t('kb.qa.form.question') }}</div>
                <div class="qa-detail__card qa-detail__card--primary">{{ qaData.question.question }}</div>
            </section>
            <section class="qa-detail__section" v-if="qaData.similarQuestions.length > 0">
                <div class="qa-detail__label">
                    {{ $t('kb.qa.form.similar') }}
                    <span class="similar-count">{{ qaData.similarQuestions.length }}</span>
                </div>
                <ul class="qa-detail__similar">
                    <li v-for="(item, idx) in qaData.similarQuestions" :key="idx" class="qa-detail__similar-item">
                        <span class="qa-detail__similar-idx">{{ idx + 1 }}</span>
                        <span class="qa-detail__similar-text">{{ item.question }}</span>
                    </li>
                </ul>
            </section>
            <section class="qa-detail__section">
                <div class="qa-detail__label">{{ $t('kb.qa.form.answer') }}</div>
                <div class="qa-detail__card qa-detail__card--answer">{{ qaData.answer }}</div>
            </section>
        </div>
        <div class="demo-drawer__footer">
            <el-button type="primary" @click="qaDetailVisible = false; editQa(qaDetailIdx)">{{ $t('common.edit') }}
            </el-button>
            <el-button type="danger" @click="deleteQa(qaDetailIdx)">{{ $t('common.del') }}</el-button>
            <el-button @click="qaDetailVisible = false">{{ $t('common.close') }}</el-button>
        </div>
    </el-drawer>
    <el-drawer v-model="dryRunFormVisible" :title="$t('kb.qa.test')" direction="rtl" size="480px">
        <el-form>
            <el-form-item label="">
                <el-input v-model="testQnAText" :placeholder="$t('kb.qa.testPH')" clearable
                    @keyup.enter="testQa" />
            </el-form-item>
            <el-form-item label="">
                <el-alert v-if="testQnAResult" :title="testQnAResult" type="info" :closable="false" />
            </el-form-item>
        </el-form>
        <div class="demo-drawer__footer">
            <el-button type="primary" :loading="loading" @click="testQa">{{ $t('kb.qa.test') }}</el-button>
            <el-button @click="dryRunFormVisible = false">{{ $t('common.close') }}</el-button>
        </div>
    </el-drawer>
</template>

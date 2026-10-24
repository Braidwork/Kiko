<script setup>
import { nextTick, onMounted, reactive, ref } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { httpReq } from '../../assets/tools.js'
import { useI18n } from 'vue-i18n'
const { t, tm, rt } = useI18n();
import RiBardLine from '~icons/ri/bard-line';
import EpPlus from '~icons/ep/plus';
const route = useRoute()
const router = useRouter();

const intentData = ref([]);
const formLabelWidth = '70px';
const dialogFormVisible = ref(false);
const dryRunFormVisible = ref(false)
const loading = ref(false)
const intentName = ref('');
const robotId = route.params.robotId;

onMounted(async () => {
    await list();
});

const goBack = () => {
    router.push({ name: 'robotDetail', params: { robotId: robotId } });
}

async function list() {
    const t = await httpReq('GET', 'intent', { robotId: robotId }, null, null);
    if (t.status == 200)
        intentData.value = t.data;
}

async function newIntent() {
    const formData = { robotId: robotId, id: '', data: intentName.value };
    const t = await httpReq('POST', 'intent', null, null, formData);
    // console.log(t.data);
    if (t.status == 200)
        await list();
}
function editIntent(idx, row) {
    router.push({ path: '/robot/' + robotId + '/intent/detail', query: { id: intentData.value[idx].intent_id, idx: idx, name: row.name } });
}
async function deleteIntent(idx, row) {
    ElMessageBox.confirm(
        t('intent.delConfirm'),
        'Warning',
        {
            confirmButtonText: t('common.del'),
            cancelButtonText: t('common.cancel'),
            type: 'warning',
        }
    ).then(async () => {
        const formData = { robotId: robotId, id: intentData.value[idx].intent_id, data: idx.toString() };
        const t = await httpReq('DELETE', 'intent', null, null, formData);
        console.log(t.data);
        if (t.status == 200) {
            await list();
            ElMessage({
                type: 'success',
                message: t('common.deleted'),
            })
        } else {
            ElMessage({
                type: 'error',
                message: t.err.message,
            })
        }
    }).catch(() => {
        // ElMessage({
        //     type: 'info',
        //     message: 'Delete canceled',
        // })
    })
}

const testIntentDetectionText = ref('')
const intentDetectResult = ref('')
function detectIntent() {
    // if (testIntentDetectionText.value == null || testIntentDetectionText.value.length < 1)
    //     return;
    // const formData = { robotId: robotId, id: '', data: testIntentDetectionText.value };
    // const t = await httpReq('POST', 'intent/detect', null, null, formData);
    // console.log(t.data);
    // if (t.status == 200) {
    //     if (t.data == null)
    //         intentDetectResult.value = 'No intention detected.';
    //     else
    //         intentDetectResult.value = 'The detected intention is: ' + t.data;
    // }
    loading.value = true;
    (async function () {
        if (testIntentDetectionText.value == null || testIntentDetectionText.value.length < 1)
            return;
        const formData = { robotId: robotId, id: '', data: testIntentDetectionText.value };
        const t = await httpReq('POST', 'intent/detect', null, null, formData);
        console.log(t.data);
        if (t.status == 200) {
            if (t.data == null)
                intentDetectResult.value = 'No intention detected.';
            else
                intentDetectResult.value = 'The detected intention is: ' + t.data;
        }
    })().then(() => loading.value = false);
}
</script>
<style scoped>
.count-badge {
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
</style>
<template>
    <div class="page-header">
        <h1 class="page-title">
            <span class="page-title-icon"><RiBardLine /></span>
            {{ $t('intent.title') }}
        </h1>
        <div class="page-actions">
            <el-button @click="dryRunFormVisible = true">{{ t('intent.test')}}</el-button>
            <el-button type="primary" @click="dialogFormVisible = true">
                <el-icon style="margin-right: 6px"><EpPlus /></el-icon>
                {{ $t('intent.add') }}
            </el-button>
        </div>
    </div>
    <div class="page-card">
        <el-table :data="intentData" stripe style="width: 100%">
            <el-table-column prop="intent_name" :label="tm('intent.table')[0]" width="220" />
            <el-table-column :label="tm('intent.table')[1]" width="160" align="center">
                <template #default="scope">
                    <span class="count-badge">{{ scope.row.keywords.length }}</span>
                </template>
            </el-table-column>
            <el-table-column :label="tm('intent.table')[2]" width="160" align="center">
                <template #default="scope">
                    <span class="count-badge">{{ scope.row.regexes.length }}</span>
                </template>
            </el-table-column>
            <el-table-column :label="tm('intent.table')[3]" width="200" align="center">
                <template #default="scope">
                    <span class="count-badge">{{ scope.row.phrases.length }}</span>
                </template>
            </el-table-column>
            <el-table-column fixed="right" :label="tm('intent.table')[4]" width="160" align="center">
                <template #default="scope">
                    <el-button link type="primary" @click="editIntent(scope.$index, scope.row)">{{
                        $t('common.edit') }}</el-button>
                    <el-button link type="danger" @click="deleteIntent(scope.$index, scope.row)">{{
                        $t('common.del') }}</el-button>
                </template>
            </el-table-column>
        </el-table>
    </div>
    <el-dialog v-model="dialogFormVisible" :title="t('intent.form.title')" width="460px" destroy-on-close>
        <el-form :model="form">
            <el-form-item :label="t('intent.form.name')" :label-width="formLabelWidth">
                <el-input v-model="intentName" autocomplete="off" :placeholder="t('intent.form.name')" />
            </el-form-item>
        </el-form>
        <template #footer>
            <el-button @click="dialogFormVisible = false">{{ $t('common.cancel') }}</el-button>
            <el-button type="primary" @click="dialogFormVisible = false; newIntent();">
                {{ $t('common.add') }}
            </el-button>
        </template>
    </el-dialog>
    <el-drawer v-model="dryRunFormVisible" :title="t('intent.test')" direction="rtl" size="480px">
        <el-form>
            <el-form-item label="">
                <el-input v-model="testIntentDetectionText" placeholder="Please input some texts" clearable />
            </el-form-item>
            <el-form-item label="">
                <el-alert v-if="intentDetectResult" :title="intentDetectResult" type="info" :closable="false" />
            </el-form-item>
        </el-form>
        <div class="demo-drawer__footer">
            <el-button type="primary" :loading="loading" @click="detectIntent">Test</el-button>
            <el-button @click="dryRunFormVisible = false">{{ $t('common.close') }}</el-button>
        </div>
    </el-drawer>
</template>
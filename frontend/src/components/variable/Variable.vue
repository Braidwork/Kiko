<script setup>
import { ref, reactive, onMounted } from 'vue';
import { useRoute, useRouter } from 'vue-router';
// import { ElMessage, ElMessageBox } from 'element-plus'
import { copyProperties, httpReq } from '../../assets/tools.js'
import { useI18n } from 'vue-i18n'
const { t, tm, rt } = useI18n();
import SolarDownloadOutline from '~icons/solar/download-outline';
import EpPlus from '~icons/ep/plus';
const route = useRoute();
const router = useRouter();
const robotId = route.params.robotId;
const varData = reactive({
    varName: '',
    varType: '',
    varValueSource: '',
    varConstantValue: '',
    varAssociateData: '',
    obtainValueExpressionType: 'None',
    obtainValueExpression: '',
    connectTimeoutMilliseconds: 100,
    readTimeoutMilliseconds: 1500,
    cacheEnabled: true,
});
const varTypes = [
    { label: tm('var.types')[0], value: 'Str' },
    { label: tm('var.types')[1], value: 'Num' },
];
const varTypesMap = new Map()
varTypes.forEach(function (item, index, arr) {
    this.set(item.value, item.label);
}, varTypesMap);

const varValueSources = [
    { label: tm('var.sources')[0], value: 'Import', disabled: false },
    { label: tm('var.sources')[1], value: 'Collect', disabled: false },
    { label: 'User input', value: 'UserInput', disabled: false },
    { label: 'Constant value', value: 'Constant', disabled: false },
    { label: tm('var.sources')[2], value: 'ExternalHttp', disabled: false },
];
const varValueSourcesMap = new Map()
varValueSources.forEach(function (item, index, arr) {
    this.set(item.value, item.label);
}, varValueSourcesMap);

const obtainValueExpressionTypes = [
    { label: 'JSON Pointer', value: 'JsonPointer', disabled: false },
    { label: 'Html Scrape', value: 'HtmlScrape', disabled: false },
]

const varSetFormVisible = ref(false);
const formLabelWidth = '160px';
const tableData = ref([])
const httpApiList = ref([])

async function list() {
    const t = await httpReq('GET', 'variable', { robotId: robotId }, null, null);
    console.log(t);
    showVars(t);
}

onMounted(async () => {
    const t = await httpReq('GET', 'external/http', { robotId: robotId }, null, null);
    // console.log(t);
    if (t && t.status == 200) {
        httpApiList.value = t.data == null ? [] : t.data;
    }
    await list();
});

const showVars = (t) => {
    if (t && t.status == 200) {
        tableData.value = t.data == null ? [] : t.data;
        tableData.value.forEach(function (item, index, arr) {
            item.varTypeT = varTypesMap.get(item.varType);
            item.varValueSourceT = varValueSourcesMap.get(item.varValueSource);
        });
    }
}

const goBack = () => {
    router.push({ name: 'robotDetail', params: { robotId: robotId } });
}

const newVar = () => {
    varData.varName = ''
    varData.varType = ''
    varData.varValueSource = ''
    varData.constantValue = ''
    varData.externalAssociateId = ''
    varData.obtainValueExpressionType = 'None'
    varData.obtainValueExpression = ''
    varData.cacheEnabled = false
    showForm()
}

const editVar = (idx, d) => {
    copyProperties(d, varData);
    // varData.varName = d.varName
    // varData.varType = d.varType
    // varData.varValueSource = d.varValueSource
    // varData.externalAssociateId = d.externalAssociateId
    // varData.obtainValueExpressionType = d.obtainValueExpressionType
    // varData.obtainValueExpression = d.obtainValueExpression
    // varData.cacheEnabled = d.cacheEnabled
    showForm()
}

const deleteVar = async (idx, d) => {
    ElMessageBox.confirm(
        d.varName + ' will be deleted permanently. Continue?',
        'Warning',
        {
            confirmButtonText: 'OK',
            cancelButtonText: 'Cancel',
            type: 'warning',
        }
    )
        .then(async () => {
            copyProperties(d, varData);
            // varData.varName = idx.toString();
            // varData.varType = d.varType
            // varData.varValueSource = d.varValueSource
            const t = await httpReq('DELETE', 'variable', { robotId: robotId }, null, varData);
            console.log(t);
            if (t.status == 200) {
                await list();
                // hideForm();
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

function showForm() {
    varSetFormVisible.value = true;
}

function hideForm() {
    varSetFormVisible.value = false;
}

async function saveForm() {
    const t = await httpReq('POST', 'variable', { robotId: robotId }, null, varData);
    console.log(t);
    await list();
    hideForm();
}
</script>
<style scoped>
.type-tag {
    display: inline-block;
    padding: 2px 12px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 600;
}

.type-tag.num {
    background: #fef3c7;
    color: #d97706;
}

.type-tag.str {
    background: #eef2ff;
    color: #6366f1;
}
</style>
<template>
    <div class="page-header">
        <h1 class="page-title">
            <span class="page-title-icon"><SolarDownloadOutline /></span>
            {{ $t('var.title') }}
        </h1>
        <div class="page-actions">
            <el-button type="primary" @click="newVar()">
                <el-icon style="margin-right: 6px"><EpPlus /></el-icon>
                {{ $t('var.add') }}
            </el-button>
        </div>
    </div>
    <div class="page-card">
        <el-table :data="tableData" stripe style="width: 100%">
            <el-table-column prop="varName" :label="tm('var.table')[0]" min-width="220" />
            <el-table-column prop="varTypeT" :label="tm('var.table')[1]" width="140" align="center">
                <template #default="scope">
                    <span class="type-tag" :class="scope.row.varType == 'Num' ? 'num' : 'str'">
                        {{ scope.row.varTypeT }}
                    </span>
                </template>
            </el-table-column>
            <el-table-column prop="varValueSourceT" :label="tm('var.table')[2]" width="180" />
            <el-table-column fixed="right" :label="tm('var.table')[3]" width="150" align="center">
                <template #default="scope">
                    <el-button link type="primary" @click="editVar(scope.$index, scope.row)">
                        {{ $t('common.edit') }}
                    </el-button>
                    <el-button link type="danger" @click="deleteVar(scope.$index, scope.row)">
                        {{ $t('common.del') }}
                    </el-button>
                </template>
            </el-table-column>
        </el-table>
    </div>
    <el-drawer v-model="varSetFormVisible" :title="$t('var.form.title')" direction="rtl" size="520px"
        :destroy-on-close="true">
        <el-form :model="varData">
            <el-form-item :label="$t('var.form.name')" :label-width="formLabelWidth">
                <el-input v-model="varData.varName" autocomplete="off" :placeholder="$t('var.form.name')" />
            </el-form-item>
            <el-form-item :label="$t('var.form.type')" :label-width="formLabelWidth">
                <el-select v-model="varData.varType" :placeholder="$t('var.form.choose1')" style="width: 100%">
                    <el-option v-for="item in varTypes" :key="item.label" :label="item.label" :value="item.value"
                        :disabled="item.disabled" />
                </el-select>
            </el-form-item>
            <el-form-item :label="$t('var.form.source')" :label-width="formLabelWidth">
                <el-select v-model="varData.varValueSource" :placeholder="$t('var.form.choose2')" style="width: 100%">
                    <el-option v-for="item in varValueSources" :key="item.label" :label="item.label"
                        :value="item.value" />
                </el-select>
            </el-form-item>
            <el-form-item v-if="varData.varValueSource == 'Constant'" label="Constant value"
                :label-width="formLabelWidth">
                <el-input v-model="varData.varConstantValue" autocomplete="on" />
            </el-form-item>
            <el-form-item v-if="varData.varValueSource == 'ExternalHttp'" label="HTTP API"
                :label-width="formLabelWidth">
                <el-select v-model="varData.varAssociateData" placeholder="Choose a HTTP API" style="width: 100%">
                    <el-option v-for="item in httpApiList" :key="item.id" :label="item.name" :value="item.id" />
                </el-select>
                <div style="margin-top: 4px;">
                    <router-link :to="{ name: 'externalHttpApiDetail', params: { robotId: robotId, id: 'new' } }">Add
                        new
                        HTTP
                        API</router-link>
                </div>
            </el-form-item>
            <el-form-item v-if="varData.varValueSource == 'ExternalHttp'" label="Value expression type"
                :label-width="formLabelWidth">
                <el-select v-model="varData.obtainValueExpressionType" placeholder="Value expression type"
                    style="width: 100%">
                    <el-option v-for="item in obtainValueExpressionTypes" :key="item.label" :label="item.label"
                        :value="item.value" />
                </el-select>
            </el-form-item>
            <el-form-item v-if="varData.varValueSource == 'ExternalHttp'" label="Obtain value expression"
                :label-width="formLabelWidth">
                <el-input v-model="varData.obtainValueExpression" autocomplete="on"
                    :placeholder="varData.obtainValueExpressionType == 'JsonPointer' ? '/data/book/name' : 'CSS selector syntax like: h1.foo div#bar'" />
            </el-form-item>
            <el-form-item v-if="varData.varValueSource == 'ExternalHttp'" label="Connect timeout" :label-width="formLabelWidth">
                <el-input-number v-model="varData.connectTimeoutMilliseconds" :min="100" :max="600000" /> milliseconds
            </el-form-item>
            <el-form-item v-if="varData.varValueSource == 'ExternalHttp'" label="Read timeout" :label-width="formLabelWidth">
                <el-input-number v-model="varData.readTimeoutMilliseconds" :min="100" :max="600000" /> milliseconds
            </el-form-item>
            <el-form-item v-if="varData.varValueSource == 'ExternalHttp'" label="Cache value"
                :label-width="formLabelWidth">
                <el-switch v-model="varData.cacheEnabled" active-text="Enable" />
            </el-form-item>
            <el-form-item v-if="varData.varValueSource == 'ExternalHttp'" label="" :label-width="formLabelWidth">
                <span v-if="varData.cacheEnabled" style="color: #86909c; font-size: 13px;">After requesting once, the
                    variable value will be stored in the cache
                    and
                    subsequently read from the cache.</span>
                <span v-if="!varData.cacheEnabled" style="color: #86909c; font-size: 13px;">HTTP API will be requested
                    every time</span>
            </el-form-item>
        </el-form>
        <div class="demo-drawer__footer">
            <el-button @click="hideForm()">{{ $t('common.cancel') }}</el-button>
            <el-button type="primary" :loading="loading" @click="saveForm()">{{ $t('common.save') }}</el-button>
        </div>
    </el-drawer>
</template>
<script setup>
import { ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute, useRouter } from 'vue-router';
import { login } from '../../assets/auth.js';

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const username = ref('');
const password = ref('');
const loading = ref(false);
const error = ref('');

async function submit() {
    error.value = '';
    if (!username.value.trim() || !password.value) {
        error.value = t('auth.required');
        return;
    }
    loading.value = true;
    try {
        await login(username.value.trim(), password.value);
        const redirect = typeof route.query.redirect === 'string' ? route.query.redirect : '/';
        // 只接受站内路径，避免把用户带到外部地址。
        await router.replace(redirect.startsWith('/') ? redirect : '/');
    } catch (e) {
        error.value = e?.message || t('auth.loginFailed');
    } finally {
        loading.value = false;
    }
}
</script>

<template>
    <div class="auth-page">
        <div class="auth-card">
            <div class="auth-brand">
                <span class="auth-logo">K</span>
                <span>Kiko</span>
            </div>
            <h1 class="auth-title">{{ t('auth.title') }}</h1>
            <p class="auth-subtitle">{{ t('auth.subtitle') }}</p>

            <el-form class="auth-form" @submit.prevent="submit">
                <el-form-item>
                    <el-input v-model="username" size="large" autocomplete="username"
                        :placeholder="t('auth.username')" @keyup.enter="submit" />
                </el-form-item>
                <el-form-item>
                    <el-input v-model="password" size="large" type="password" show-password
                        autocomplete="current-password" :placeholder="t('auth.password')"
                        @keyup.enter="submit" />
                </el-form-item>
            </el-form>

            <el-alert v-if="error" class="auth-error" :title="error" type="error" :closable="false" show-icon />
            <el-button class="auth-submit" type="primary" size="large" :loading="loading" @click="submit">
                {{ loading ? t('auth.signingIn') : t('auth.signIn') }}
            </el-button>
            <p class="auth-foot">{{ t('auth.footer') }}</p>
        </div>
    </div>
</template>

<style scoped>
.auth-page {
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    background:
        radial-gradient(circle at 85% 15%, rgba(139, 92, 246, 0.28), transparent 55%),
        radial-gradient(circle at 10% 90%, rgba(56, 189, 248, 0.22), transparent 55%),
        linear-gradient(135deg, #eef2ff, #f8fafc 55%, #eef2ff);
}

.auth-card {
    width: 100%;
    max-width: 400px;
    padding: 40px 36px 32px;
    border-radius: 20px;
    background: rgba(255, 255, 255, 0.92);
    box-shadow: 0 24px 60px rgba(31, 45, 61, 0.16);
    backdrop-filter: blur(14px);
}

.auth-brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 700;
    font-size: 18px;
    color: #1f2d3d;
}

.auth-logo {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 10px;
    background: linear-gradient(135deg, #6366f1, #8b5cf6);
    color: #fff;
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.4);
}

.auth-title {
    margin: 26px 0 6px;
    font-size: 24px;
    color: #1f2d3d;
}

.auth-subtitle {
    margin: 0 0 22px;
    font-size: 13px;
    color: #86909c;
}

.auth-form :deep(.el-form-item) {
    margin-bottom: 16px;
}

.auth-error {
    margin-bottom: 14px;
}

.auth-submit {
    width: 100%;
    border: none;
    font-weight: 600;
    border-radius: 10px;
}

.auth-foot {
    margin-top: 22px;
    text-align: center;
    font-size: 12px;
    color: #a0a6b1;
}
</style>

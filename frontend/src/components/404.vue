<script setup>
import { computed } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import EpHomeFilled from "~icons/ep/home-filled";
import EpBack from "~icons/ep/back";

const route = useRoute();
const router = useRouter();
const { t, locale } = useI18n();

// The path the user tried to open, e.g. "/foo/bar"
const requestedPath = computed(() => route.fullPath.replace(/\?.*$/, ""));
const canGoBack = computed(() => window.history.length > 1);

function goBack() {
    if (canGoBack.value) router.back();
    else router.push("/");
}

function toLang(lang) {
    if (lang === locale.value) return;
    locale.value = lang;
    localStorage.setItem("lang", lang);
}
</script>

<template>
    <div class="not-found">
        <div class="nf-decor nf-decor--ring"></div>
        <div class="nf-decor nf-decor--square"></div>

        <div class="nf-langs">
            <button type="button" class="nf-lang" :class="{ 'is-active': locale === 'en' }" @click="toLang('en')">
                English
            </button>
            <span class="nf-lang-divider"></span>
            <button type="button" class="nf-lang" :class="{ 'is-active': locale === 'zh' }" @click="toLang('zh')">
                简体中文
            </button>
        </div>

        <section class="nf-card">
            <span class="nf-badge">{{ t("notFound.badge") }}</span>

            <h1 class="nf-code" aria-hidden="true">404</h1>

            <h2 class="nf-title">{{ t("notFound.title") }}</h2>
            <p class="nf-desc">{{ t("notFound.desc") }}</p>

            <p class="nf-path">
                <span class="nf-path-label">{{ t("notFound.pathLabel") }}</span>
                <code>{{ requestedPath }}</code>
            </p>

            <div class="nf-actions">
                <el-button type="primary" size="large" class="nf-btn" @click="router.push('/')">
                    <el-icon class="nf-btn-icon">
                        <EpHomeFilled />
                    </el-icon>
                    {{ t("notFound.backHome") }}
                </el-button>
                <el-button v-if="canGoBack" size="large" class="nf-btn" @click="goBack">
                    <el-icon class="nf-btn-icon">
                        <EpBack />
                    </el-icon>
                    {{ t("notFound.goBack") }}
                </el-button>
            </div>

            <p class="nf-hint">{{ t("notFound.hint") }}</p>
        </section>
    </div>
</template>

<style scoped>
.not-found {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    padding: 40px 20px;
    overflow: hidden;
    background:
        radial-gradient(1000px 520px at 12% -10%, rgba(99, 102, 241, 0.16), transparent 60%),
        radial-gradient(900px 480px at 88% 108%, rgba(139, 92, 246, 0.16), transparent 60%),
        var(--app-bg);
}

/* ---------- floating decorations ---------- */
.nf-decor {
    position: absolute;
    border-radius: 50%;
    pointer-events: none;
    animation: nf-float 9s ease-in-out infinite;
}

.nf-decor--ring {
    top: 12%;
    left: 10%;
    width: 170px;
    height: 170px;
    border: 2px dashed rgba(99, 102, 241, 0.28);
}

.nf-decor--square {
    right: 11%;
    bottom: 14%;
    width: 120px;
    height: 120px;
    border-radius: 30px;
    background: linear-gradient(135deg, rgba(99, 102, 241, 0.14), rgba(139, 92, 246, 0.12));
    animation-delay: -4.5s;
}

@keyframes nf-float {
    0%,
    100% {
        transform: translateY(0) rotate(0deg);
    }
    50% {
        transform: translateY(-18px) rotate(8deg);
    }
}

/* ---------- language switcher ---------- */
.nf-langs {
    position: absolute;
    top: 26px;
    right: 30px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 5px;
    border: 1px solid var(--app-card-border);
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.78);
    backdrop-filter: blur(8px);
    box-shadow: 0 2px 10px rgba(31, 45, 61, 0.06);
}

.nf-lang {
    padding: 6px 14px;
    border: none;
    border-radius: 999px;
    background: transparent;
    color: var(--app-text-secondary);
    font-size: 13px;
    line-height: 1;
    cursor: pointer;
    transition: background 0.2s, color 0.2s;
}

.nf-lang:hover {
    color: var(--app-primary);
    background: #eef2ff;
}

.nf-lang.is-active {
    color: #fff;
    background: linear-gradient(135deg, #6366f1, #8b5cf6);
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.32);
}

.nf-lang-divider {
    width: 1px;
    height: 14px;
    background: var(--app-card-border);
}

/* ---------- card ---------- */
.nf-card {
    position: relative;
    z-index: 1;
    width: 100%;
    max-width: 560px;
    padding: 46px 40px 40px;
    border: 1px solid var(--app-card-border);
    border-radius: 22px;
    background: rgba(255, 255, 255, 0.92);
    backdrop-filter: blur(10px);
    box-shadow: 0 22px 60px rgba(31, 45, 61, 0.1);
    text-align: center;
    animation: nf-rise 0.5s cubic-bezier(0.22, 1, 0.36, 1) both;
}

@keyframes nf-rise {
    from {
        opacity: 0;
        transform: translateY(18px) scale(0.98);
    }
    to {
        opacity: 1;
        transform: translateY(0) scale(1);
    }
}

.nf-badge {
    display: inline-block;
    padding: 5px 14px;
    border: 1px solid #e0e7ff;
    border-radius: 999px;
    background: #eef2ff;
    color: var(--app-primary);
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.12em;
    text-transform: uppercase;
}

.nf-code {
    margin: 14px 0 0;
    font-size: 118px;
    font-weight: 800;
    line-height: 1.05;
    letter-spacing: 0.06em;
    background: linear-gradient(135deg, #6366f1 0%, #8b5cf6 45%, #a855f7 100%);
    -webkit-background-clip: text;
    background-clip: text;
    -webkit-text-fill-color: transparent;
    color: transparent;
    text-shadow: 0 14px 32px rgba(99, 102, 241, 0.18);
    animation: nf-pulse 3.6s ease-in-out infinite;
}

@keyframes nf-pulse {
    0%,
    100% {
        transform: translateY(0);
        filter: drop-shadow(0 8px 18px rgba(99, 102, 241, 0.16));
    }
    50% {
        transform: translateY(-6px);
        filter: drop-shadow(0 16px 26px rgba(139, 92, 246, 0.24));
    }
}

.nf-title {
    margin: 2px 0 0;
    font-size: 22px;
    font-weight: 700;
    color: var(--app-text);
}

.nf-desc {
    margin: 10px auto 0;
    max-width: 400px;
    font-size: 14px;
    line-height: 1.7;
    color: var(--app-text-secondary);
}

.nf-path {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    max-width: 100%;
    margin: 18px 0 0;
    padding: 8px 14px;
    border: 1px dashed #dfe3ec;
    border-radius: 10px;
    background: #f9fafb;
    font-size: 12.5px;
    color: var(--app-text-secondary);
}

.nf-path-label {
    flex-shrink: 0;
    color: #9aa3b2;
}

.nf-path code {
    overflow: hidden;
    max-width: 320px;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color: #4b5563;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.nf-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: center;
    gap: 12px;
    margin-top: 28px;
}

.nf-btn {
    border-radius: 10px;
    font-weight: 600;
}

.nf-btn-icon {
    margin-right: 6px;
}

.nf-hint {
    margin: 22px 0 0;
    font-size: 12px;
    color: #9aa3b2;
}

/* ---------- responsive ---------- */
@media (max-width: 640px) {
    .nf-card {
        padding: 34px 22px 30px;
        border-radius: 18px;
    }

    .nf-code {
        font-size: 84px;
    }

    .nf-title {
        font-size: 19px;
    }

    .nf-decor {
        display: none;
    }

    .nf-langs {
        top: 16px;
        right: 16px;
    }

    .nf-path code {
        max-width: 180px;
    }
}
</style>

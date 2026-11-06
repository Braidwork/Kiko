-- 0008_external_http_global_inline_params.sql
--
-- 1. external_http_apis 改为全局（对所有机器人可见），去掉 robot_id；
-- 2. 参数从 external_http_params 内联进主表：
--      headers             JSONB 数组 [{name,value,valueSource}]
--      query_string        JSONB 数组 [{name,value,valueSource}]
--      urlencoded_payload  JSONB 数组 [{name,value,valueSource}]
--      payload             原始请求体 TEXT（原 request_body）
--    valueSource 的取值统一为 Const / Variable。
-- 3. 新项目不做旧参数数据迁移：external_http_params 直接删除。
--
-- 已存在的 API 行保留；request_body 改名后值不变，旧的 header/query/form
-- 参数不再保留（需要时重新录入）。

DROP TABLE IF EXISTS external_http_params;

ALTER TABLE external_http_apis DROP COLUMN IF EXISTS robot_id;
ALTER TABLE external_http_apis RENAME COLUMN request_body TO payload;

UPDATE external_http_apis SET payload = '' WHERE payload IS NULL;
ALTER TABLE external_http_apis ALTER COLUMN payload SET DEFAULT '';
ALTER TABLE external_http_apis ALTER COLUMN payload SET NOT NULL;

UPDATE external_http_apis SET post_content_type = 'Raw' WHERE post_content_type IS NULL;
ALTER TABLE external_http_apis ALTER COLUMN post_content_type SET DEFAULT 'Raw';
ALTER TABLE external_http_apis ALTER COLUMN post_content_type SET NOT NULL;

ALTER TABLE external_http_apis
    ADD COLUMN headers JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN query_string JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN urlencoded_payload JSONB NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE external_http_apis
    ADD CONSTRAINT external_http_apis_headers_array
        CHECK (jsonb_typeof(headers) = 'array'),
    ADD CONSTRAINT external_http_apis_query_string_array
        CHECK (jsonb_typeof(query_string) = 'array'),
    ADD CONSTRAINT external_http_apis_urlencoded_payload_array
        CHECK (jsonb_typeof(urlencoded_payload) = 'array');

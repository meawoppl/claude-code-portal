// @generated automatically by Diesel CLI.

diesel::table! {
    messages (id) {
        id -> Uuid,
        session_id -> Uuid,
        #[max_length = 50]
        role -> Varchar,
        content -> Text,
        created_at -> Timestamp,
        user_id -> Uuid,
    }
}

diesel::table! {
    pending_permission_requests (id) {
        id -> Uuid,
        session_id -> Uuid,
        #[max_length = 255]
        request_id -> Varchar,
        #[max_length = 255]
        tool_name -> Varchar,
        input -> Jsonb,
        permission_suggestions -> Nullable<Jsonb>,
        created_at -> Timestamp,
    }
}

diesel::table! {
    proxy_auth_tokens (id) {
        id -> Uuid,
        user_id -> Uuid,
        #[max_length = 255]
        name -> Varchar,
        #[max_length = 64]
        token_hash -> Varchar,
        created_at -> Timestamp,
        last_used_at -> Nullable<Timestamp>,
        expires_at -> Timestamp,
        revoked -> Bool,
    }
}

diesel::table! {
    sessions (id) {
        id -> Uuid,
        user_id -> Uuid,
        #[max_length = 255]
        session_name -> Varchar,
        #[max_length = 255]
        session_key -> Varchar,
        working_directory -> Nullable<Text>,
        #[max_length = 50]
        status -> Varchar,
        last_activity -> Timestamp,
        created_at -> Timestamp,
        updated_at -> Timestamp,
        #[max_length = 255]
        git_branch -> Nullable<Varchar>,
        total_cost_usd -> Float8,
    }
}

diesel::table! {
    users (id) {
        id -> Uuid,
        #[max_length = 255]
        google_id -> Varchar,
        #[max_length = 255]
        email -> Varchar,
        #[max_length = 255]
        name -> Nullable<Varchar>,
        avatar_url -> Nullable<Text>,
        created_at -> Timestamp,
        updated_at -> Timestamp,
        is_admin -> Bool,
        disabled -> Bool,
    }
}

diesel::joinable!(messages -> sessions (session_id));
diesel::joinable!(messages -> users (user_id));
diesel::joinable!(pending_permission_requests -> sessions (session_id));
diesel::joinable!(proxy_auth_tokens -> users (user_id));
diesel::joinable!(sessions -> users (user_id));

diesel::allow_tables_to_appear_in_same_query!(
    messages,
    pending_permission_requests,
    proxy_auth_tokens,
    sessions,
    users,
);

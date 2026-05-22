// @generated automatically by Diesel CLI.
//
// Hand edit kept after `diesel print-schema`:
//   `cargo_args` from `Array<Nullable<Text>>` → `Array<Text>`. Postgres
//   permits NULL elements in TEXT[] but we never insert them, so the
//   application type stays `Option<Vec<String>>`.

diesel::table! {
    build_logs (id) {
        id -> Uuid,
        program_id -> Text,
        file_name -> Text,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    builds (id) {
        id -> Uuid,
        repository -> Text,
        commit_hash -> Nullable<Text>,
        program_id -> Text,
        lib_name -> Nullable<Text>,
        base_docker_image -> Nullable<Text>,
        mount_path -> Nullable<Text>,
        cargo_args -> Nullable<Array<Text>>,
        bpf_flag -> Bool,
        arch -> Nullable<Text>,
        signer -> Nullable<Text>,
        status -> Text,
        executable_hash -> Nullable<Text>,
        error_message -> Nullable<Text>,
        created_at -> Timestamptz,
        completed_at -> Nullable<Timestamptz>,
    }
}

diesel::table! {
    program_state (program_id) {
        program_id -> Text,
        on_chain_hash -> Nullable<Text>,
        authority -> Nullable<Text>,
        is_frozen -> Bool,
        is_closed -> Bool,
        last_checked -> Timestamptz,
    }
}

diesel::allow_tables_to_appear_in_same_query!(build_logs, builds, program_state,);

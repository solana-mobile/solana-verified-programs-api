// @generated automatically by Diesel CLI.

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
        // Hand-edited: Postgres TEXT[] permits NULL elements, but we never
        // insert NULLs. Mapping as `Array<Text>` avoids forcing every call
        // site to handle a `Vec<Option<String>>`.
        cargo_args -> Nullable<Array<Text>>,
        bpf_flag -> Bool,
        created_at -> Timestamptz,
        status -> Text,
        signer -> Nullable<Text>,
        arch -> Nullable<Text>,
        executable_hash -> Nullable<Text>,
        error_message -> Nullable<Text>,
        completed_at -> Nullable<Timestamptz>,
    }
}

diesel::table! {
    program_state (program_id) {
        program_id -> Text,
        authority -> Nullable<Text>,
        last_checked -> Timestamptz,
        is_frozen -> Nullable<Bool>,
        is_closed -> Bool,
        on_chain_hash -> Nullable<Text>,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    build_logs,
    builds,
    program_state,
);

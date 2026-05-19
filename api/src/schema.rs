// @generated automatically by Diesel CLI.

diesel::table! {
    build_logs (id) {
        #[max_length = 36]
        id -> Varchar,
        #[max_length = 44]
        program_address -> Varchar,
        file_name -> Varchar,
        created_at -> Timestamp,
    }
}

diesel::table! {
    program_authority (program_id) {
        #[max_length = 44]
        program_id -> Varchar,
        #[max_length = 44]
        authority_id -> Nullable<Varchar>,
        last_updated -> Timestamp,
        is_frozen -> Bool,
        is_closed -> Bool,
    }
}

diesel::table! {
    solana_program_builds (id) {
        #[max_length = 36]
        id -> Varchar,
        repository -> Varchar,
        commit_hash -> Nullable<Varchar>,
        #[max_length = 44]
        program_id -> Varchar,
        lib_name -> Nullable<Varchar>,
        base_docker_image -> Nullable<Varchar>,
        mount_path -> Nullable<Varchar>,
        cargo_args -> Nullable<Array<Text>>,
        bpf_flag -> Bool,
        created_at -> Timestamp,
        #[max_length = 20]
        status -> Varchar,
        signer -> Nullable<Varchar>,
        #[max_length = 3]
        arch -> Nullable<Varchar>,
    }
}

diesel::table! {
    verified_hashes (executable_hash) {
        executable_hash -> Varchar,
        repository -> Varchar,
        commit_hash -> Nullable<Varchar>,
        lib_name -> Nullable<Varchar>,
        base_docker_image -> Nullable<Varchar>,
        mount_path -> Nullable<Varchar>,
        cargo_args -> Nullable<Array<Text>>,
        bpf_flag -> Bool,
        #[max_length = 3]
        arch -> Nullable<Varchar>,
        verified_at -> Timestamp,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    build_logs,
    program_authority,
    solana_program_builds,
    verified_hashes,
);

// Ports: gorm.io/gorm schema/naming_test.go (TestToDBName, TestNamingStrategy, TestCustomReplacer, TestCustomReplacerWithNoLowerCase, TestFormatNameWithStringLongerThan63Characters, TestFormatNameWithStringLongerThan64Characters, TestReplaceEmptyTableName) @ v1.31.1 (the version add21b5's go.mod pins)
//
// Derived from GORM: Copyright (c) 2013-present Jinzhu; MIT license in
// crates/gorse-storage-core/licenses/GORM-LICENSE.

//! gorm's `naming_test.go`, case for case, with these differences:
//!
//! - TestToDBName's second map (`SchemaName`) is not ported, because
//!   `SchemaName` is not; its first map (`toDBName`) is.
//! - The Go tests set `SingularTable: true`; the port is singular only and has
//!   no such field.
//! - The custom replacers call `strings.ToUpper`, ported as
//!   `to_ascii_uppercase`, which is the same on ASCII; each one asserts its
//!   input is ASCII.
//! - Go ranges over its maps in random order; the cases here run in source
//!   order.

mod schema {
    use gorse_common::gostrings;
    use gorse_storage_core::gorm_naming::NamingStrategy;
    use gorse_storage_core::gorm_naming::internal;

    fn s(name: Vec<u8>) -> String {
        String::from_utf8(name).expect("gorm name is valid UTF-8")
    }

    // `"REPLACED_" + strings.ToUpper(name)` through
    // `strings.NewReplacer("CID", "_Cid")`, the replacer both custom replacer
    // tests use.
    fn replace_upper(name: &[u8]) -> Vec<u8> {
        assert!(name.is_ascii(), "ToUpper ported for ASCII only: {name:?}");
        let mut replaced = b"REPLACED_".to_vec();
        replaced.extend_from_slice(&name.to_ascii_uppercase());
        gostrings::Replacer::new(&[("CID", "_Cid")]).replace(replaced)
    }

    #[test]
    fn test_to_db_name() {
        let maps = [
            ("", ""),
            ("x", "x"),
            ("X", "x"),
            ("userRestrictions", "user_restrictions"),
            ("ThisIsATest", "this_is_a_test"),
            ("PFAndESI", "pf_and_esi"),
            ("AbcAndJkl", "abc_and_jkl"),
            ("EmployeeID", "employee_id"),
            ("SKU_ID", "sku_id"),
            ("FieldX", "field_x"),
            ("HTTPAndSMTP", "http_and_smtp"),
            (
                "HTTPServerHandlerForURLID",
                "http_server_handler_for_url_id",
            ),
            ("UUID", "uuid"),
            ("HTTPURL", "http_url"),
            ("HTTP_URL", "http_url"),
            ("SHA256Hash", "sha256_hash"),
            ("SHA256HASH", "sha256_hash"),
            (
                "ThisIsActuallyATestSoWeMayBeAbleToUseThisCodeInGormPackageAlsoIdCanBeUsedAtTheEndAsID",
                "this_is_actually_a_test_so_we_may_be_able_to_use_this_code_in_gorm_package_also_id_can_be_used_at_the_end_as_id",
            ),
        ];

        let ns = NamingStrategy::default();
        for (key, value) in maps {
            let got = s(internal::to_db_name(&ns, key.as_bytes()));
            assert_eq!(
                got, value,
                "{key} toName should equal {value}, but got {got}"
            );
        }
    }

    #[test]
    fn test_naming_strategy() {
        let ns = NamingStrategy {
            table_prefix: b"public.".to_vec(),
            name_replacer: Some(Box::new(gostrings::Replacer::new(&[("CID", "Cid")]))),
            ..NamingStrategy::default()
        };
        let idx_name = s(ns.index_name("public.table", "name"));
        assert_eq!(
            idx_name, "idx_public_table_name",
            "invalid index name generated, got {idx_name}"
        );

        let chk_name = s(ns.checker_name("public.table", "name"));
        assert_eq!(
            chk_name, "chk_public_table_name",
            "invalid checker name generated, got {chk_name}"
        );

        let join_table = s(ns.join_table_name("user_languages"));
        assert_eq!(
            join_table, "public.user_languages",
            "invalid join table generated, got {join_table}"
        );

        let join_table2 = s(ns.join_table_name("UserLanguage"));
        assert_eq!(
            join_table2, "public.user_language",
            "invalid join table generated, got {join_table2}"
        );

        let table_name = s(ns.table_name("Company"));
        assert_eq!(
            table_name, "public.company",
            "invalid table name generated, got {table_name}"
        );

        let columd_name = s(ns.column_name("", "NameCID"));
        assert_eq!(
            columd_name, "name_cid",
            "invalid column name generated, got {columd_name}"
        );
    }

    #[test]
    fn test_custom_replacer() {
        let ns = NamingStrategy {
            table_prefix: b"public.".to_vec(),
            name_replacer: Some(Box::new(replace_upper)),
            no_lower_case: false,
            ..NamingStrategy::default()
        };

        let idx_name = s(ns.index_name("public.table", "name"));
        assert_eq!(
            idx_name, "idx_public_table_replaced_name",
            "invalid index name generated, got {idx_name}"
        );

        let chk_name = s(ns.checker_name("public.table", "name"));
        assert_eq!(
            chk_name, "chk_public_table_name",
            "invalid checker name generated, got {chk_name}"
        );

        // Seems like a bug in NamingStrategy to skip the Replacer when the
        // name is lowercase here.
        let join_table = s(ns.join_table_name("user_languages"));
        assert_eq!(
            join_table, "public.user_languages",
            "invalid join table generated, got {join_table}"
        );

        let join_table2 = s(ns.join_table_name("UserLanguage"));
        assert_eq!(
            join_table2, "public.replaced_userlanguage",
            "invalid join table generated, got {join_table2}"
        );

        let table_name = s(ns.table_name("Company"));
        assert_eq!(
            table_name, "public.replaced_company",
            "invalid table name generated, got {table_name}"
        );

        let columd_name = s(ns.column_name("", "NameCID"));
        assert_eq!(
            columd_name, "replaced_name_cid",
            "invalid column name generated, got {columd_name}"
        );
    }

    #[test]
    fn test_custom_replacer_with_no_lower_case() {
        let ns = NamingStrategy {
            table_prefix: b"public.".to_vec(),
            name_replacer: Some(Box::new(replace_upper)),
            no_lower_case: true,
            ..NamingStrategy::default()
        };

        let idx_name = s(ns.index_name("public.table", "name"));
        assert_eq!(
            idx_name, "idx_public_table_REPLACED_NAME",
            "invalid index name generated, got {idx_name}"
        );

        let chk_name = s(ns.checker_name("public.table", "name"));
        assert_eq!(
            chk_name, "chk_public_table_name",
            "invalid checker name generated, got {chk_name}"
        );

        let join_table = s(ns.join_table_name("user_languages"));
        assert_eq!(
            join_table, "public.REPLACED_USER_LANGUAGES",
            "invalid join table generated, got {join_table}"
        );

        let join_table2 = s(ns.join_table_name("UserLanguage"));
        assert_eq!(
            join_table2, "public.REPLACED_USERLANGUAGE",
            "invalid join table generated, got {join_table2}"
        );

        let table_name = s(ns.table_name("Company"));
        assert_eq!(
            table_name, "public.REPLACED_COMPANY",
            "invalid table name generated, got {table_name}"
        );

        let columd_name = s(ns.column_name("", "NameCID"));
        assert_eq!(
            columd_name, "REPLACED_NAME_Cid",
            "invalid column name generated, got {columd_name}"
        );
    }

    #[test]
    fn test_format_name_with_string_longer_than_63_characters() {
        let ns = NamingStrategy {
            identifier_max_length: 63,
            ..NamingStrategy::default()
        };

        let formatted_name = s(internal::format_name(
            &ns,
            b"prefix",
            b"table",
            b"thisIsAVeryVeryVeryVeryVeryVeryVeryVeryVeryLongString",
        ));
        assert_eq!(
            formatted_name, "prefix_table_thisIsAVeryVeryVeryVeryVeryVeryVeryVeryVer180f2c67",
            "invalid formatted name generated, got {formatted_name}"
        );
    }

    #[test]
    fn test_format_name_with_string_longer_than_64_characters() {
        let ns = NamingStrategy {
            identifier_max_length: 64,
            ..NamingStrategy::default()
        };

        let formatted_name = s(internal::format_name(
            &ns,
            b"prefix",
            b"table",
            b"thisIsAVeryVeryVeryVeryVeryVeryVeryVeryVeryLongString",
        ));
        assert_eq!(
            formatted_name, "prefix_table_thisIsAVeryVeryVeryVeryVeryVeryVeryVeryVery180f2c67",
            "invalid formatted name generated, got {formatted_name}"
        );
    }

    #[test]
    fn test_replace_empty_table_name() {
        let ns = NamingStrategy {
            name_replacer: Some(Box::new(gostrings::Replacer::new(&[("Model", "")]))),
            ..NamingStrategy::default()
        };
        let table_name = s(ns.table_name("Model"));
        assert_eq!(
            table_name, "Model",
            "invalid table name generated, got {table_name}"
        );
    }
}

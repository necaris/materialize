// Copyright Materialize, Inc. and contributors. All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

// BEGIN LINT CONFIG
// DO NOT EDIT. Automatically generated by bin/gen-lints.
// Have complaints about the noise? See the note in misc/python/materialize/cli/gen-lints.py first.
#![allow(unknown_lints)]
#![allow(clippy::style)]
#![allow(clippy::complexity)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::mutable_key_type)]
#![allow(clippy::stable_sort_primitive)]
#![allow(clippy::map_entry)]
#![allow(clippy::box_default)]
#![allow(clippy::drain_collect)]
#![warn(clippy::bool_comparison)]
#![warn(clippy::clone_on_ref_ptr)]
#![warn(clippy::no_effect)]
#![warn(clippy::unnecessary_unwrap)]
#![warn(clippy::dbg_macro)]
#![warn(clippy::todo)]
#![warn(clippy::wildcard_dependencies)]
#![warn(clippy::zero_prefixed_literal)]
#![warn(clippy::borrowed_box)]
#![warn(clippy::deref_addrof)]
#![warn(clippy::double_must_use)]
#![warn(clippy::double_parens)]
#![warn(clippy::extra_unused_lifetimes)]
#![warn(clippy::needless_borrow)]
#![warn(clippy::needless_question_mark)]
#![warn(clippy::needless_return)]
#![warn(clippy::redundant_pattern)]
#![warn(clippy::redundant_slicing)]
#![warn(clippy::redundant_static_lifetimes)]
#![warn(clippy::single_component_path_imports)]
#![warn(clippy::unnecessary_cast)]
#![warn(clippy::useless_asref)]
#![warn(clippy::useless_conversion)]
#![warn(clippy::builtin_type_shadow)]
#![warn(clippy::duplicate_underscore_argument)]
#![warn(clippy::double_neg)]
#![warn(clippy::unnecessary_mut_passed)]
#![warn(clippy::wildcard_in_or_patterns)]
#![warn(clippy::crosspointer_transmute)]
#![warn(clippy::excessive_precision)]
#![warn(clippy::overflow_check_conditional)]
#![warn(clippy::as_conversions)]
#![warn(clippy::match_overlapping_arm)]
#![warn(clippy::zero_divided_by_zero)]
#![warn(clippy::must_use_unit)]
#![warn(clippy::suspicious_assignment_formatting)]
#![warn(clippy::suspicious_else_formatting)]
#![warn(clippy::suspicious_unary_op_formatting)]
#![warn(clippy::mut_mutex_lock)]
#![warn(clippy::print_literal)]
#![warn(clippy::same_item_push)]
#![warn(clippy::useless_format)]
#![warn(clippy::write_literal)]
#![warn(clippy::redundant_closure)]
#![warn(clippy::redundant_closure_call)]
#![warn(clippy::unnecessary_lazy_evaluations)]
#![warn(clippy::partialeq_ne_impl)]
#![warn(clippy::redundant_field_names)]
#![warn(clippy::transmutes_expressible_as_ptr_casts)]
#![warn(clippy::unused_async)]
#![warn(clippy::disallowed_methods)]
#![warn(clippy::disallowed_macros)]
#![warn(clippy::disallowed_types)]
#![warn(clippy::from_over_into)]
// END LINT CONFIG

//! Integration tests for balancerd.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use jsonwebtoken::{DecodingKey, EncodingKey};
use mz_balancerd::{BalancerConfig, BalancerService, FronteggResolver, Resolver, BUILD_INFO};
use mz_environmentd::test_util::{self, make_pg_tls, Ca};
use mz_frontegg_auth::{
    Authentication as FronteggAuthentication, AuthenticationConfig as FronteggConfig,
};
use mz_frontegg_mock::FronteggMockServer;
use mz_ore::metrics::MetricsRegistry;
use mz_ore::now::SYSTEM_TIME;
use mz_ore::task::RuntimeExt;
use mz_server_core::TlsCertConfig;
use openssl::ssl::{SslConnectorBuilder, SslVerifyMode};
use postgres::Client;
use uuid::Uuid;

#[mz_ore::test]
#[cfg_attr(miri, ignore)] // too slow
fn test_balancer() {
    let ca = Ca::new_root("test ca").unwrap();
    let (server_cert, server_key) = ca
        .request_cert("server", vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
        .unwrap();
    let metrics_registry = MetricsRegistry::new();

    let tenant_id = Uuid::new_v4();
    let client_id = Uuid::new_v4();
    let secret = Uuid::new_v4();
    let users = BTreeMap::from([(
        (client_id.to_string(), secret.to_string()),
        "user@_.com".to_string(),
    )]);
    let roles = BTreeMap::from([("user@_.com".to_string(), Vec::new())]);
    let encoding_key =
        EncodingKey::from_rsa_pem(&ca.pkey.private_key_to_pem_pkcs8().unwrap()).unwrap();

    const EXPIRES_IN_SECS: i64 = 50;
    let frontegg_server = FronteggMockServer::start(
        None,
        encoding_key,
        tenant_id,
        users,
        roles,
        SYSTEM_TIME.clone(),
        EXPIRES_IN_SECS,
        None,
    )
    .unwrap();
    let frontegg_auth = FronteggAuthentication::new(
        FronteggConfig {
            admin_api_token_url: frontegg_server.url.clone(),
            decoding_key: DecodingKey::from_rsa_pem(&ca.pkey.public_key_to_pem().unwrap()).unwrap(),
            tenant_id: Some(tenant_id),
            now: SYSTEM_TIME.clone(),
            admin_role: "mzadmin".to_string(),
        },
        mz_frontegg_auth::Client::default(),
        &metrics_registry,
    );
    let frontegg_user = "user@_.com";
    let frontegg_password = format!("mzp_{client_id}{secret}");

    let config = test_util::Config::default()
        // Enable SSL on the main port. There should be a balancerd port with no SSL.
        .with_tls(server_cert.clone(), server_key.clone())
        .with_frontegg(&frontegg_auth)
        .with_metrics_registry(metrics_registry);
    let envd_server = test_util::start_server(config).unwrap();

    // Ensure we could connect directly to envd without SSL on the balancer port.
    let mut pg_client_envd = envd_server
        .pg_config_balancer()
        .user(frontegg_user)
        .password(&frontegg_password)
        .connect(tokio_postgres::NoTls)
        .unwrap();
    let res: i32 = pg_client_envd.query_one("SELECT 4", &[]).unwrap().get(0);
    assert_eq!(res, 4);

    let resolvers = vec![
        Resolver::Static(envd_server.inner.balancer_sql_local_addr().to_string()),
        Resolver::Frontegg(FronteggResolver {
            auth: frontegg_auth,
            addr_template: envd_server.inner.balancer_sql_local_addr().to_string(),
        }),
    ];
    let cert_config = Some(TlsCertConfig {
        cert: server_cert,
        key: server_key,
    });

    for resolver in resolvers {
        let balancer_cfg = BalancerConfig::new(
            &BUILD_INFO,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            resolver,
            envd_server.inner.http_local_addr().to_string(),
            cert_config.clone(),
            MetricsRegistry::new(),
        );
        let balancer_server = envd_server
            .runtime
            .block_on(async { BalancerService::new(balancer_cfg).await.unwrap() });
        let balancer_pgwire_listen = balancer_server.pgwire.0.local_addr();
        envd_server.runtime.spawn_named(|| "balancer", async {
            balancer_server.serve().await.unwrap();
        });

        let mut pg_client = Client::connect(
            &format!(
                "user={frontegg_user} password={frontegg_password} host={} port={} sslmode=require",
                balancer_pgwire_listen.ip(),
                balancer_pgwire_listen.port()
            ),
            make_pg_tls(Box::new(|b: &mut SslConnectorBuilder| {
                Ok(b.set_verify(SslVerifyMode::NONE))
            })),
        )
        .unwrap();

        let res: i32 = pg_client.query_one("SELECT 2", &[]).unwrap().get(0);
        assert_eq!(res, 2);
    }
}

load("@aspect_bazel_lib//lib:expand_template.bzl", "expand_template")
load("@crates//:defs.bzl", "all_crate_deps")
load("@rules_oci//oci:defs.bzl", "oci_image", "oci_load", "oci_push")
load("@rules_pkg//pkg:tar.bzl", "pkg_tar")
load("@rules_rust//rust:defs.bzl", "rust_binary", "rust_library", "rust_test", "rust_test_suite")

package(default_visibility = ["//visibility:public"])

# https://github.com/bazelbuild/rules_rust/issues/2701
config_setting(
    name = "release",
    values = {
        "compilation_mode": "opt",
    },
)


#####
rust_binary(
        name = "staff",  # Remove the .rs extension
        srcs = ["src/main.rs", "src/staff.rs"],
        data = [],
        edition = "2021",
        rustc_flags = select({
            "//:release": [
                "-Clto",
                "-Ccodegen-units=1",
                "-Cpanic=abort",
                "-Copt-level=3",
                "-Cstrip=symbols",
            ],
            "//conditions:default": [
                "-Copt-level=0",
            ],
        }),
        visibility = ["//visibility:public"],
        deps = [

        ] + all_crate_deps(),
    )


rust_test(
    name = "unit_tests",
    crate = ":staff",
    visibility = ["//visibility:public"],
)


pkg_tar(
    name = "server_tar",
    srcs = ["//:staff"],
    package_dir = "/opt/staff/bin/",
)

pkg_tar(
    name = "deployment",
    deps = [
        ":server_tar",
    ],
)

oci_image(
    name = "staff-image",
    base = "@base_image",
    cmd = [],
    entrypoint = ["/opt/staff/bin/staff"],
    tars = [":deployment"],
    visibility = ["//visibility:public"],
)

expand_template(
    name = "stamped",
    out = "_stamped.tags.txt",
    stamp_substitutions = {"0.0.0": "{{CONTAINER_TAG}}"},
    template = ["0.0.0"],
)

# oci_push(
#     name = "staff-image-push",
#     image = ":staff-image",
#     remote_tags = ":stamped",
#     visibility = ["//visibility:public"],
#     repository = "example.com/staff:tag",
# )

oci_load(
    name = "staff-image-load",
    image = ":staff-image",
    repo_tags = ["staff:latest"],
    visibility = ["//visibility:public"],
)

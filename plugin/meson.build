project('nix-btm-plugin', 'cpp',
  version : '0.1.0',
  default_options : ['cpp_std=c++23', 'warning_level=2'],
)

nix_util_dep = dependency('nix-util')
nix_main_dep = dependency('nix-main')
boost_dep = dependency('boost', include_type : 'system')

# Embed the compile-time nix version so the plugin can report mismatches.
nix_version = nix_util_dep.version()
add_project_arguments(
  '-DNIX_BTM_COMPILED_NIX_VERSION="' + nix_version + '"',
  language : 'cpp',
)

shared_library('nix-btm',
  'src/btm-plugin.cc',
  dependencies : [nix_util_dep, nix_main_dep, boost_dep],
  install : true,
)

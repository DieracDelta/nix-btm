project('nix-analytics-plugin', 'cpp',
  version : '0.1.0',
  default_options : ['cpp_std=c++23', 'warning_level=2'],
)

nix_util_dep = dependency('nix-util')
nix_main_dep = dependency('nix-main')
boost_dep = dependency('boost', include_type : 'system')

shared_library('nix-analytics',
  'src/analytics-plugin.cc',
  dependencies : [nix_util_dep, nix_main_dep, boost_dep],
  install : true,
)

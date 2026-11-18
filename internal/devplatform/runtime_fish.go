package devplatform

import (
	"path/filepath"
	"strings"
)

func fishQuote(s string) string {
	return "'" + strings.NewReplacer("\\", "\\\\", "'", "\\'").Replace(s) + "'"
}

func runtimeFishScript(name, binary, jvmDir string) string {
	if name != "java" {
		return `# Written by Devbase. Only the current process environment changes.
set -l msm_bin ` + fishQuote(filepath.Dir(binary)) + `
set -l msm_path "$msm_bin"
for entry in $PATH
  if test "$entry" != "$msm_bin"
    set -a msm_path "$entry"
  end
end
set -gx PATH $msm_path
`
	}
	defaultBin := filepath.Dir(binary)
	defaultHome := filepath.Dir(defaultBin)
	return `# Written by Devbase. Changes are overwritten.
set -g MSM_JAVA_DEFAULT_HOME ` + fishQuote(defaultHome) + `
set -g MSM_JAVA_DEFAULT_BIN ` + fishQuote(defaultBin) + `
set -g MSM_JAVA_JVM_DIR ` + fishQuote(jvmDir) + `

functions -e __msm_java_apply
function __msm_java_apply --on-variable PWD
  set -l dir "$PWD"
  if test -z "$dir"
    set dir (pwd)
  end
  set -l spec ""
  while test -n "$dir"
    if test -r "$dir/.java-version"
      while read -l line
        set line (string replace -r '#.*' '' -- "$line" | string trim)
        if test -n "$line"
          set spec "$line"
          break
        end
      end < "$dir/.java-version"
    end
    if test -z "$spec"; and test -r "$dir/.sdkmanrc"
      while read -l line
        set line (string replace -r '#.*' '' -- "$line" | string trim)
        if string match -rq '^java\s*=' -- "$line"
          set -l val (string replace -r '^java\s*=\s*' '' -- "$line" | string trim)
          set val (string trim -c '"' -- "$val")
          set val (string trim -c "'" -- "$val")
          if test -n "$val"
            set spec "$val"
            break
          end
        end
      end < "$dir/.sdkmanrc"
    end
    if test -n "$spec"; or test "$dir" = "/"
      break
    end
    set dir (string replace -r '/[^/]*$' '' -- "$dir")
    if test -z "$dir"
      set dir "/"
    end
  end

  set -l home ""
  set -l bin ""
  if test -n "$spec"; and test -n "$MSM_JAVA_JVM_DIR"; and test -d "$MSM_JAVA_JVM_DIR"
    set -l sdk_ver ""
    set -l sdk_vendor ""
    set -l sdk_distro ""
    set -l distro_spec ""
    if string match -rq '^[0-9].*-[A-Za-z]' -- "$spec"
      set sdk_ver (string replace -r '-.*$' '' -- "$spec")
      set sdk_vendor (string lower -- (string replace -r '^[^-]*-' '' -- "$spec"))
      switch "$sdk_vendor"
        case tem temurin eclipse adoptium
          set sdk_distro temurin
        case amzn corretto amazon
          set sdk_distro corretto
        case zulu azul
          set sdk_distro zulu
        case sem semeru ibm
          set sdk_distro semeru
        case graal graalce graalvm
          set sdk_distro graalvm
        case librca liberica bellsoft
          set sdk_distro liberica
        case sapmchn sapmachine
          set sdk_distro sapmachine
        case ms microsoft
          set sdk_distro ms
        case oracle
          set sdk_distro oracle
        case open openjdk
          set sdk_distro openjdk
        case jbr jetbrains
          set sdk_distro jbr
        case '*'
          set sdk_distro "$sdk_vendor"
      end
      set distro_spec "$sdk_distro-$sdk_ver"
    end

    set -l match1 ""
    set -l match2 ""
    set -l match2_prefix ""
    set -l match3 ""
    set -l match4 ""
    for base in (command ls -1 -- "$MSM_JAVA_JVM_DIR" 2>/dev/null)
      set -l cand "$MSM_JAVA_JVM_DIR/$base"
      test -d "$cand"; or continue
      set -l cand_home ""
      if test -x "$cand/Contents/Home/bin/java"
        set cand_home "$cand/Contents/Home"
      else if test -x "$cand/bin/java"
        set cand_home "$cand"
      else
        continue
      end
      set -l java_ver ""
      set -l os_arch ""
      set -l implementor ""
      if test -r "$cand_home/release"
        while read -l line
          set line (string trim -- "$line")
          if string match -q 'JAVA_VERSION=*' -- "$line"
            set java_ver (string replace 'JAVA_VERSION=' '' -- "$line" | string trim -c '"')
          else if string match -q 'OS_ARCH=*' -- "$line"
            set os_arch (string replace 'OS_ARCH=' '' -- "$line" | string trim -c '"')
          else if string match -q 'IMPLEMENTOR=*' -- "$line"
            set implementor (string replace 'IMPLEMENTOR=' '' -- "$line" | string trim -c '"')
          end
        end < "$cand_home/release"
      end
      if test -n "$os_arch"; and test "$os_arch" != "aarch64"
        continue
      end

      if test "$base" = "$spec"
        set match1 "$cand_home"
        break
      end
      if test -z "$match2"
        if test "$base" = "$spec.jdk"
          set match2 "$cand_home"
        else if test -n "$distro_spec"; and begin; test "$base" = "$distro_spec"; or test "$base" = "$distro_spec.jdk"; end
          set match2 "$cand_home"
        end
      end
      if test -z "$match2_prefix"
        if string match -q -- "$spec.*" "$base"; or string match -q -- "$spec-*" "$base"; or string match -q -- "$spec""_*" "$base"; or string match -q -- "$spec+*" "$base"
          set match2_prefix "$cand_home"
        else if test -n "$distro_spec"
          if string match -q -- "$distro_spec.*" "$base"; or string match -q -- "$distro_spec-*" "$base"; or string match -q -- "$distro_spec""_*" "$base"; or string match -q -- "$distro_spec+*" "$base"
            set match2_prefix "$cand_home"
          end
        end
      end
      if test "$os_arch" = "aarch64"; and test -n "$java_ver"
        if test -z "$match3"; and test "$java_ver" = "$spec"
          set match3 "$cand_home"
        else if test -z "$sdk_ver"; and test -z "$match4"
          if string match -q -- "$spec.*" "$java_ver"; or string match -q -- "$spec+*" "$java_ver"; or string match -q -- "$spec""_*" "$java_ver"; or string match -q -- "$spec-*" "$java_ver"
            set match4 "$cand_home"
          end
        else if test -n "$sdk_ver"
          set -l imp_lower (string lower -- "$implementor")
          set -l vendor_ok 0
          switch "$sdk_distro"
            case temurin
              string match -rq 'eclipse|adoptium|temurin|adoptopenjdk' -- "$imp_lower"; and set vendor_ok 1
            case corretto
              string match -rq 'amazon|corretto' -- "$imp_lower"; and set vendor_ok 1
            case zulu
              string match -rq 'azul|zulu' -- "$imp_lower"; and set vendor_ok 1
            case semeru
              string match -rq 'ibm|semeru' -- "$imp_lower"; and set vendor_ok 1
            case graalvm
              string match -rq 'graal' -- "$imp_lower"; and set vendor_ok 1
            case liberica
              string match -rq 'bellsoft|liberica' -- "$imp_lower"; and set vendor_ok 1
            case sapmachine
              string match -rq 'sap' -- "$imp_lower"; and set vendor_ok 1
            case ms
              string match -rq 'microsoft' -- "$imp_lower"; and set vendor_ok 1
            case oracle
              string match -rq 'oracle' -- "$imp_lower"; and set vendor_ok 1
            case openjdk
              string match -rq 'openjdk|homebrew' -- "$imp_lower"; and set vendor_ok 1
            case jbr
              string match -rq 'jetbrains' -- "$imp_lower"; and set vendor_ok 1
          end
          if test "$vendor_ok" -eq 1
            if test -z "$match3"; and test "$java_ver" = "$sdk_ver"
              set match3 "$cand_home"
            else if test -z "$match4"
              if string match -q -- "$sdk_ver.*" "$java_ver"; or string match -q -- "$sdk_ver+*" "$java_ver"; or string match -q -- "$sdk_ver""_*" "$java_ver"; or string match -q -- "$sdk_ver-*" "$java_ver"
                set match4 "$cand_home"
              end
            end
          end
        end
      end
    end

    for resolved in "$match1" "$match2" "$match2_prefix" "$match3" "$match4"
      if test -n "$resolved"
        set home "$resolved"
        set bin "$resolved/bin"
        break
      end
    end
  end

  if test -z "$home"
    set home "$MSM_JAVA_DEFAULT_HOME"
    set bin "$MSM_JAVA_DEFAULT_BIN"
  end

  set -l remaining
  for entry in $PATH
    if test "$entry" != "$bin"; and begin; not set -q __MSM_JAVA_BIN; or test "$entry" != "$__MSM_JAVA_BIN"; end
      set -a remaining "$entry"
    end
  end
  set -gx PATH "$bin" $remaining
  set -gx JAVA_HOME "$home"
  set -gx __MSM_JAVA_BIN "$bin"
end
__msm_java_apply
`
}

func (m *Manager) nodeFishScript() string {
	return `# Written by Devbase. Changes are overwritten.
set -g MSM_NODE_RESOLVE ` + fishQuote(m.nodeWrapperPath()) + `
set -gx MSM_NODE_HOME ` + fishQuote(m.nodeHome()) + `
functions -e __msm_node_apply
function __msm_node_apply --on-variable PWD
  set -l selection (command "$MSM_NODE_RESOLVE" node path "$PWD")
  set -l previous (string split : -- (string join : -- $__MSM_NODE_PATH))
  set -l remaining
  for entry in $PATH
    if not contains -- "$entry" $previous
      set -a remaining "$entry"
    end
  end
  set -l selected
  if test (count $selection) -ge 2
    set selected (string split : -- "$selection[1]")
    set -gx npm_config_prefix "$selection[2]"
  else
    set -e npm_config_prefix
  end
  set -gx PATH $selected $remaining
  set -gx __MSM_NODE_PATH $selected
end
__msm_node_apply
`
}

package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
)

func validAdditionalRuntime(name string) bool { return name == "java" }
func (m *Manager) runtimeScript(name string) string {
	return filepath.Join(m.Root, "runtimes", name, "init.sh")
}
func shellLiteral(s string) string { return "'" + strings.ReplaceAll(s, "'", "'\\''") + "'" }
func (m *Manager) SetRuntimeDefault(ctx context.Context, name, id string) error {
	if !validAdditionalRuntime(name) {
		return errors.New("unsupported runtime")
	}
	return m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
		p, ok := s.Packages[id]
		if !ok || p.Name != name {
			return nil, nil, errors.New("choose an installed runtime version")
		}
		binary, e := m.Binary(s, id, name)
		if e != nil {
			return nil, nil, e
		}
		return m.saveRuntimeDefault(s, name, id, binary)
	})
}
func (m *Manager) saveRuntimeDefault(s *State, name, id, binary string) (func() error, func(), error) {
	undo, e := m.updateRuntimeScripts(name, []string{runtimeInitScript(name, binary, m.javaDirectory()), runtimeFishScript(name, binary, m.javaDirectory())})
	if e != nil {
		return nil, nil, e
	}
	if s.RuntimeDefaults == nil {
		s.RuntimeDefaults = map[string]string{}
	}
	s.RuntimeDefaults[name] = id
	return undo, nil, nil
}

func (m *Manager) ConfigureRuntimeShell(ctx context.Context, name string, enable bool) error {
	if !validAdditionalRuntime(name) {
		return errors.New("unsupported runtime")
	}
	if e := ctx.Err(); e != nil {
		return e
	}
	if enable {
		// Refresh existing init scripts too, under the same state transaction
		// as default selection so an older selection cannot overwrite a new one.
		e := m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
			id := s.RuntimeDefaults[name]
			if id == "" {
				return nil, nil, errors.New("select a default version first")
			}
			binary, e := m.runtimeBinary(s, id, name)
			if e != nil {
				return nil, nil, e
			}
			return m.saveRuntimeDefault(s, name, id, binary)
		})
		if e != nil {
			return e
		}
	}
	begin := "# BEGIN Devbase " + name
	end := "# END Devbase " + name
	_, e := m.configureRuntimeShellFiles(ctx, name, begin, end, enable)
	if e != nil {
		return e
	}
	if !enable {
		if _, err := m.updateRuntimeScripts(name, nil); err != nil {
			return err
		}
	}
	return nil
}

func runtimeInitScript(name, binary, jvmDir string) string {
	if name != "java" {
		return runtimePathScript(filepath.Dir(binary))
	}
	defaultBin := filepath.Dir(binary)
	defaultHome := filepath.Dir(defaultBin)
	return `# Written by Devbase. Changes are overwritten.
MSM_JAVA_DEFAULT_HOME=` + shellLiteral(defaultHome) + `
MSM_JAVA_DEFAULT_BIN=` + shellLiteral(defaultBin) + `
MSM_JAVA_JVM_DIR=` + shellLiteral(jvmDir) + `

__msm_java_apply() {
  local dir spec line val home bin cand cand_home java_ver os_arch implementor
  local sdk_ver sdk_vendor sdk_distro distro_spec base
  local match1 match2 match2_prefix match3 match4 resolved rest item clean have old_bin
  [ -n "${ZSH_VERSION-}" ] && setopt local_options null_glob
  dir="${PWD:-$(pwd)}"
  spec=""
  while [ -n "$dir" ]; do
    if [ -r "$dir/.java-version" ]; then
      while IFS= read -r line || [ -n "$line" ]; do
        line="${line%%#*}"
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line%"${line##*[![:space:]]}"}"
        if [ -n "$line" ]; then
          spec="$line"
          break
        fi
      done < "$dir/.java-version"
    fi
    if [ -z "$spec" ] && [ -r "$dir/.sdkmanrc" ]; then
      while IFS= read -r line || [ -n "$line" ]; do
        line="${line%%#*}"
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line%"${line##*[![:space:]]}"}"
        case "$line" in
          java=*) val="${line#java=}" ;;
          java[[:space:]]*=*) val="${line#*=}" ;;
          *) continue ;;
        esac
        val="${val#"${val%%[![:space:]]*}"}"
        val="${val%"${val##*[![:space:]]}"}"
        case "$val" in
          \"*\") val="${val#\"}"; val="${val%\"}" ;;
          \'*\') val="${val#\'}"; val="${val%\'}" ;;
        esac
        if [ -n "$val" ]; then
          spec="$val"
          break
        fi
      done < "$dir/.sdkmanrc"
    fi
    if [ -n "$spec" ] || [ "$dir" = "/" ]; then
      break
    fi
    dir="${dir%/*}"
    [ -n "$dir" ] || dir="/"
  done

  home=""
  bin=""
  if [ -n "$spec" ] && [ -n "$MSM_JAVA_JVM_DIR" ] && [ -d "$MSM_JAVA_JVM_DIR" ]; then
    sdk_ver=""
    sdk_vendor=""
    sdk_distro=""
    distro_spec=""
    case "$spec" in
      [0-9]*-[A-Za-z]*)
        sdk_ver="${spec%%-*}"
        sdk_vendor="${spec#*-}"
        case "$sdk_vendor" in
          [Tt][Ee][Mm]|[Tt][Ee][Mm][Uu][Rr][Ii][Nn]|[Ee][Cc][Ll][Ii][Pp][Ss][Ee]|[Aa][Dd][Oo][Pp][Tt][Ii][Uu][Mm]) sdk_distro="temurin" ;;
          [Aa][Mm][Zz][Nn]|[Cc][Oo][Rr][Rr][Ee][Tt][Tt][Oo]|[Aa][Mm][Aa][Zz][Oo][Nn]) sdk_distro="corretto" ;;
          [Zz][Uu][Ll][Uu]|[Aa][Zz][Uu][Ll]) sdk_distro="zulu" ;;
          [Ss][Ee][Mm]|[Ss][Ee][Mm][Ee][Rr][Uu]|[Ii][Bb][Mm]) sdk_distro="semeru" ;;
          [Gg][Rr][Aa][Aa][Ll]|[Gg][Rr][Aa][Aa][Ll][Cc][Ee]|[Gg][Rr][Aa][Aa][Ll][Vv][Mm]) sdk_distro="graalvm" ;;
          [Ll][Ii][Bb][Rr][Cc][Aa]|[Ll][Ii][Bb][Ee][Rr][Ii][Cc][Aa]|[Bb][Ee][Ll][Ll][Ss][Oo][Ff][Tt]) sdk_distro="liberica" ;;
          [Ss][Aa][Pp][Mm][Cc][Hh][Nn]|[Ss][Aa][Pp][Mm][Aa][Cc][Hh][Ii][Nn][Ee]) sdk_distro="sapmachine" ;;
          [Mm][Ss]|[Mm][Ii][Cc][Rr][Oo][Ss][Oo][Ff][Tt]) sdk_distro="ms" ;;
          [Oo][Rr][Aa][Cc][Ll][Ee]) sdk_distro="oracle" ;;
          [Oo][Pp][Ee][Nn]|[Oo][Pp][Ee][Nn][Jj][Dd][Kk]) sdk_distro="openjdk" ;;
          [Jj][Bb][Rr]|[Jj][Ee][Tt][Bb][Rr][Aa][Ii][Nn][Ss]) sdk_distro="jbr" ;;
          *) sdk_distro="$sdk_vendor" ;;
        esac
        distro_spec="$sdk_distro-$sdk_ver"
        ;;
    esac

    match1=""
    match2=""
    match2_prefix=""
    match3=""
    match4=""
    for cand in "$MSM_JAVA_JVM_DIR"/*; do
      [ -d "$cand" ] || continue
      base="${cand##*/}"
      case "$base" in .* ) continue ;; esac
      if [ -x "$cand/Contents/Home/bin/java" ]; then
        cand_home="$cand/Contents/Home"
      elif [ -x "$cand/bin/java" ]; then
        cand_home="$cand"
      else
        continue
      fi
      java_ver=""
      os_arch=""
      implementor=""
      if [ -r "$cand_home/release" ]; then
        while IFS= read -r line || [ -n "$line" ]; do
          line="${line%"${line##*[![:space:]]}"}"
          case "$line" in
            JAVA_VERSION=*)
              val="${line#JAVA_VERSION=}"
              case "$val" in \"*\") val="${val#\"}"; val="${val%\"}" ;; esac
              java_ver="$val"
              ;;
            OS_ARCH=*)
              val="${line#OS_ARCH=}"
              case "$val" in \"*\") val="${val#\"}"; val="${val%\"}" ;; esac
              os_arch="$val"
              ;;
            IMPLEMENTOR=*)
              val="${line#IMPLEMENTOR=}"
              case "$val" in \"*\") val="${val#\"}"; val="${val%\"}" ;; esac
              implementor="$val"
              ;;
          esac
        done < "$cand_home/release"
      fi
      if [ -n "$os_arch" ] && [ "$os_arch" != "aarch64" ]; then
        continue
      fi

      if [ "$base" = "$spec" ]; then
        match1="$cand_home"
        break
      fi
      if [ -z "$match2" ]; then
        case "$base" in
          "$spec".jdk) match2="$cand_home" ;;
          *)
            if [ -n "$distro_spec" ]; then
              case "$base" in
                "$distro_spec"|"$distro_spec".jdk) match2="$cand_home" ;;
              esac
            fi
            ;;
        esac
      fi
      if [ -z "$match2_prefix" ]; then
        case "$base" in
          "$spec".*|"$spec"-*|"$spec"_*|"$spec"+*) match2_prefix="$cand_home" ;;
          *)
            if [ -n "$distro_spec" ]; then
              case "$base" in
                "$distro_spec".*|"$distro_spec"-*|"$distro_spec"_*|"$distro_spec"+*) match2_prefix="$cand_home" ;;
              esac
            fi
            ;;
        esac
      fi
      if [ "$os_arch" = "aarch64" ] && [ -n "$java_ver" ]; then
        if [ -z "$match3" ] && [ "$java_ver" = "$spec" ]; then
          match3="$cand_home"
        elif [ -z "$sdk_ver" ] && [ -z "$match4" ]; then
          case "$java_ver" in
            "$spec".*|"$spec"+*|"$spec"_*|"$spec"-*) match4="$cand_home" ;;
          esac
        elif [ -n "$sdk_ver" ]; then
          case "$sdk_distro:$implementor" in
            temurin:*[Ee][Cc][Ll][Ii][Pp][Ss][Ee]*|temurin:*[Aa][Dd][Oo][Pp][Tt][Ii][Uu][Mm]*|temurin:*[Tt][Ee][Mm][Uu][Rr][Ii][Nn]*|\
            corretto:*[Aa][Mm][Aa][Zz][Oo][Nn]*|corretto:*[Cc][Oo][Rr][Rr][Ee][Tt][Tt][Oo]*|\
            zulu:*[Aa][Zz][Uu][Ll]*|zulu:*[Zz][Uu][Ll][Uu]*|\
            semeru:*[Ii][Bb][Mm]*|semeru:*[Ss][Ee][Mm][Ee][Rr][Uu]*|\
            graalvm:*[Gg][Rr][Aa][Aa][Ll]*|\
            liberica:*[Bb][Ee][Ll][Ll][Ss][Oo][Ff][Tt]*|liberica:*[Ll][Ii][Bb][Ee][Rr][Ii][Cc][Aa]*|\
            sapmachine:*[Ss][Aa][Pp]*|\
            ms:*[Mm][Ii][Cc][Rr][Oo][Ss][Oo][Ff][Tt]*|\
            oracle:*[Oo][Rr][Aa][Cc][Ll][Ee]*|\
            openjdk:*[Oo][Pp][Ee][Nn][Jj][Dd][Kk]*|openjdk:*[Hh][Oo][Mm][Ee][Bb][Rr][Ee][Ww]*|\
            jbr:*[Jj][Ee][Tt][Bb][Rr][Aa][Ii][Nn][Ss]*)
              if [ -z "$match3" ] && [ "$java_ver" = "$sdk_ver" ]; then
                match3="$cand_home"
              elif [ -z "$match4" ]; then
                case "$java_ver" in
                  "$sdk_ver".*|"$sdk_ver"+*|"$sdk_ver"_*|"$sdk_ver"-*) match4="$cand_home" ;;
                esac
              fi
              ;;
          esac
        fi
      fi
    done

    for resolved in "$match1" "$match2" "$match2_prefix" "$match3" "$match4"; do
      if [ -n "$resolved" ]; then
        home="$resolved"
        bin="$resolved/bin"
        break
      fi
    done
  fi

  if [ -z "$home" ]; then
    home="$MSM_JAVA_DEFAULT_HOME"
    bin="$MSM_JAVA_DEFAULT_BIN"
  fi

  old_bin="${__MSM_JAVA_BIN-}"
  rest="$PATH"
  clean=""
  have=""
  while :; do
    item="${rest%%:*}"
    if [ "$item" = "$bin" ] || { [ -n "$old_bin" ] && [ "$item" = "$old_bin" ]; }; then
      :
    else
      clean="$clean${have:+:}$item"
      have=1
    fi
    case "$rest" in *:*) rest="${rest#*:}" ;; *) break ;; esac
  done
  export PATH="$bin${have:+:$clean}"
  export JAVA_HOME="$home"
  export __MSM_JAVA_BIN="$bin"
  __MSM_JAVA_PWD="$PWD"
}

if [ -n "${ZSH_VERSION-}" ]; then
  autoload -Uz add-zsh-hook 2>/dev/null && add-zsh-hook chpwd __msm_java_apply
elif [ -n "${BASH_VERSION-}" ]; then
  declare -F java >/dev/null || java() { __msm_java_apply; command java "$@"; }
  declare -F javac >/dev/null || javac() { __msm_java_apply; command javac "$@"; }
  case $- in
    *i*)
      __msm_java_prompt() {
        [ "$PWD" = "${__MSM_JAVA_PWD-}" ] && return 0
        __msm_java_apply
      }
      case ";${PROMPT_COMMAND-};" in
        *";__msm_java_prompt;"*) ;;
        *) PROMPT_COMMAND="__msm_java_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
      esac
      ;;
  esac
fi
__msm_java_apply
`
}

// Move the selected bin to the front even after macOS path_helper runs.
// Preserve empty PATH entries and avoid duplicates in nested shells.
func runtimePathScript(bin string) string {
	return `__msm_runtime_path() {
  local bin rest item result
  bin=` + shellLiteral(bin) + `
  rest="$PATH"
  result="$bin"
  while :; do
    item="${rest%%:*}"
    [ "$item" = "$bin" ] || result="$result:$item"
    case "$rest" in *:*) rest="${rest#*:}" ;; *) break ;; esac
  done
  export PATH="$result"
}
__msm_runtime_path
unset -f __msm_runtime_path
`
}

// Both shell formats track the same default and roll back together on failure.
// A nil scripts slice removes the default's setup files.
func (m *Manager) updateRuntimeScripts(name string, scripts []string) (func() error, error) {
	type snapshot struct {
		path   string
		data   []byte
		exists bool
	}
	var snapshots []snapshot
	for _, suffix := range []string{".sh", ".fish"} {
		path := strings.TrimSuffix(m.runtimeScript(name), ".sh") + suffix
		data, err := os.ReadFile(path)
		if err != nil && !errors.Is(err, os.ErrNotExist) {
			return nil, err
		}
		snapshots = append(snapshots, snapshot{path, data, err == nil})
	}
	written := 0
	undo := func() error {
		var result error
		for i := written - 1; i >= 0; i-- {
			old := snapshots[i]
			var err error
			if old.exists {
				err = atomicWrite(old.path, old.data, 0600)
			} else {
				err = os.Remove(old.path)
				if errors.Is(err, os.ErrNotExist) {
					err = nil
				}
			}
			result = errors.Join(result, err)
		}
		return result
	}
	for i, old := range snapshots {
		var err error
		if scripts == nil {
			err = os.Remove(old.path)
			if errors.Is(err, os.ErrNotExist) {
				err = nil
			}
		} else {
			err = atomicWrite(old.path, []byte(scripts[i]), 0600)
		}
		if err != nil {
			return nil, errors.Join(err, undo())
		}
		written++
	}
	return undo, nil
}

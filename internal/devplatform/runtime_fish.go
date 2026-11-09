package devplatform

import (
	"path/filepath"
	"strings"
)

func fishQuote(s string) string {
	return "'" + strings.NewReplacer("\\", "\\\\", "'", "\\'").Replace(s) + "'"
}

func runtimeFishScript(name, binary string) string {
	script := `# Written by Devbase. Only the current process environment changes.
set -l msm_bin ` + fishQuote(filepath.Dir(binary)) + `
set -l msm_path "$msm_bin"
for entry in $PATH
  if test "$entry" != "$msm_bin"
    set -a msm_path "$entry"
  end
end
set -gx PATH $msm_path
`
	if name == "java" {
		script += "set -gx JAVA_HOME " + fishQuote(filepath.Dir(filepath.Dir(binary))) + "\n"
	}
	return script
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

//go:build !darwin

package devplatform

import "errors"

func exchangePackageDirectories(string, string) error {
	return errors.New("atomic package repair requires macOS")
}

package devplatform

import (
	"context"
	"errors"
	"net"
	"net/url"
	"strconv"
	"strings"
)

// ConnectionProfile is instance-owned address metadata. Credentials are never stored here.
type ConnectionProfile struct {
	ID              string `json:"id"`
	InstanceID      string `json:"instanceId"`
	Name            string `json:"name"`
	Database        string `json:"database,omitempty"`
	Username        string `json:"username,omitempty"`
	MongoAuthSource string `json:"mongoAuthSource,omitempty"`
	RedisDB         int    `json:"redisDb,omitempty"`
}

func profileText(value string, required bool) bool {
	if len(value) > 160 || required && strings.TrimSpace(value) == "" {
		return false
	}
	return !strings.ContainsAny(value, "\x00\r\n\t")
}

func validateConnectionProfile(p ConnectionProfile, kind string) error {
	if validID(p.ID) != nil || validID(p.InstanceID) != nil || !profileText(p.Name, true) {
		return errors.New("invalid connection profile identity")
	}
	if !profileText(p.Database, kind != "redis") || !profileText(p.Username, kind != "redis") || !profileText(p.MongoAuthSource, false) {
		return errors.New("invalid connection profile field")
	}
	switch kind {
	case "mysql", "postgresql":
		if p.MongoAuthSource != "" || p.RedisDB != 0 {
			return errors.New("unsupported connection profile option")
		}
	case "mongodb":
		if p.RedisDB != 0 || p.MongoAuthSource == "" {
			return errors.New("MongoDB requires an authentication database")
		}
	case "redis":
		if p.Username != "" && p.Username != "default" || p.Database != "" || p.MongoAuthSource != "" || p.RedisDB < 0 || p.RedisDB > 15 {
			return errors.New("Redis supports only default user and database 0–15")
		}
	default:
		return errors.New("unsupported database engine")
	}
	return nil
}

func (m *Manager) SaveConnectionProfile(ctx context.Context, profile ConnectionProfile) (ConnectionProfile, error) {
	if profile.ID == "" {
		profile.ID = randomID()
	}
	err := m.withState(ctx, func(s *State) error {
		instance, ok := s.Instances[profile.InstanceID]
		if !ok {
			return errors.New("database instance not found")
		}
		if err := validateConnectionProfile(profile, instance.Kind); err != nil {
			return err
		}
		if previous, ok := s.ConnectionProfiles[profile.ID]; ok && previous.InstanceID != profile.InstanceID {
			return errors.New("connection profile belongs to another instance")
		}
		s.ConnectionProfiles[profile.ID] = profile
		return nil
	})
	return profile, err
}

func (m *Manager) DeleteConnectionProfile(ctx context.Context, instanceID, profileID string) error {
	return m.withState(ctx, func(s *State) error {
		profile, ok := s.ConnectionProfiles[profileID]
		if !ok || profile.InstanceID != instanceID {
			return errors.New("connection profile not found")
		}
		delete(s.ConnectionProfiles, profileID)
		return nil
	})
}

func removeInstanceProfiles(s *State, instanceID string) {
	for id, profile := range s.ConnectionProfiles {
		if profile.InstanceID == instanceID {
			delete(s.ConnectionProfiles, id)
		}
	}
}

func (m *Manager) ProfileConnectionInfo(instanceID, profileID string) (ConnectionInfo, error) {
	s, err := m.Read()
	if err != nil {
		return ConnectionInfo{}, err
	}
	instance, ok := s.Instances[instanceID]
	if !ok || instance.Port < 1024 || instance.Port > 65535 {
		return ConnectionInfo{}, errors.New("database instance unavailable")
	}
	profile, ok := s.ConnectionProfiles[profileID]
	if !ok || profile.InstanceID != instanceID {
		return ConnectionInfo{}, errors.New("connection profile not found")
	}
	if err := validateConnectionProfile(profile, instance.Kind); err != nil {
		return ConnectionInfo{}, err
	}
	query := url.Values{}
	uri := url.URL{Host: net.JoinHostPort("127.0.0.1", strconv.Itoa(instance.Port))}
	user := profile.Username
	switch instance.Kind {
	case "mysql", "postgresql":
		uri.Scheme, uri.Path = instance.Kind, "/"+profile.Database
	case "mongodb":
		uri.Scheme, uri.Path = "mongodb", "/"+profile.Database
		query.Set("authSource", profile.MongoAuthSource)
		query.Set("directConnection", "true")
	case "redis":
		uri.Scheme, uri.Path = "redis", "/"+strconv.Itoa(profile.RedisDB)
		user = "default"
	}
	uri.User = url.User(user)
	uri.RawQuery = query.Encode()
	return ConnectionInfo{URI: uri.String(), Username: user}, nil
}

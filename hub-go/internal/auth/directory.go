package auth

import (
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"strings"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

const (
	settingDirectory = "directory"
	settingMockSeed  = "mockldap.seeded"
	// SourceMock marks users of the mock LDAP directory in the users table.
	SourceMock = "mockldap"
)

// storedDirectory reads the administrator's override, if any.
func (s *Service) storedDirectory() (config.Directory, bool) {
	v, ok := s.Store.Setting(settingDirectory)
	if !ok {
		return config.Directory{}, false
	}
	var d config.Directory
	if json.Unmarshal([]byte(v), &d) != nil {
		return config.Directory{}, false
	}
	return d, true
}

// Directory is the settings in force: what the administrator saved, else the environment. The bind password
// falls back to the environment's when the saved one is empty. Mode is resolved, defaults applied.
func (s *Service) Directory() config.Directory {
	d, stored := s.storedDirectory()
	if !stored {
		d = s.Cfg.Directory
	} else if d.BindPassword == "" && d.BindDN == s.Cfg.Directory.BindDN {
		d.BindPassword = s.Cfg.Directory.BindPassword
	}
	if stored && strings.TrimSpace(d.Mode) == "" {
		d.Mode = s.Cfg.Directory.Mode
	}
	d.Mode = d.EffectiveMode()
	return d
}

// DirectoryMode is mockldap, ldap or local.
func (s *Service) DirectoryMode() string { return s.Directory().Mode }

func dirName(mode string) string {
	switch mode {
	case "ldap":
		return "LDAP"
	case "mockldap":
		return "Mock LDAP"
	}
	return "Local"
}

// DirectorySettings is what an API may show: no bind password, only whether there is one.
type DirectorySettings struct {
	Mode            string   `json:"mode"`
	Host            string   `json:"host"`
	Port            int      `json:"port"`
	SSL             bool     `json:"ssl"`
	StartTLS        bool     `json:"startTls"`
	SkipCertCheck   bool     `json:"skipCertCheck"`
	TimeoutSeconds  int      `json:"timeoutSeconds"`
	BindDN          string   `json:"bindDn"`
	BindPasswordSet bool     `json:"bindPasswordSet"`
	UserDNTemplate  string   `json:"userDnTemplate"`
	UserSearchBase  string   `json:"userSearchBase"`
	UserFilter      string   `json:"userFilter"`
	GroupSearchBase string   `json:"groupSearchBase"`
	GroupFilter     string   `json:"groupFilter"`
	UseMemberOf     bool     `json:"useMemberOf"`
	DisplayNameAttr string   `json:"displayNameAttr"`
	AdminGroup      string   `json:"adminGroup"`
	AllowedGroups   []string `json:"allowedGroups"`
}

func viewOf(d config.Directory) DirectorySettings {
	if d.AllowedGroups == nil {
		d.AllowedGroups = []string{}
	}
	return DirectorySettings{Mode: d.Mode, Host: d.Host, Port: d.Port, SSL: d.SSL, StartTLS: d.StartTLS, SkipCertCheck: d.SkipCertCheck,
		TimeoutSeconds: d.TimeoutSeconds, BindDN: d.BindDN, BindPasswordSet: d.BindPassword != "", UserDNTemplate: d.UserDNTemplate,
		UserSearchBase: d.UserSearchBase, UserFilter: d.UserFilter, GroupSearchBase: d.GroupSearchBase, GroupFilter: d.GroupFilter,
		UseMemberOf: d.UseMemberOf, DisplayNameAttr: d.DisplayNameAttr, AdminGroup: d.AdminGroup, AllowedGroups: d.AllowedGroups}
}

// DirectoryInput is the PUT body (and the "settings" of a test): the settings plus the secret handling. BindPassword
// absent or empty keeps the current secret; a value replaces it; ClearBindPassword removes it.
type DirectoryInput struct {
	config.Directory
	ClearBindPassword bool `json:"clearBindPassword"`
}

// SaveDirectory validates and stores the administrator's settings (replacing the environment's as a whole).
func (s *Service) SaveDirectory(in DirectoryInput) error {
	d := in.Directory
	d.Mode = strings.ToLower(strings.TrimSpace(d.Mode))
	if err := ValidateDirectory(d); err != nil {
		return err
	}
	switch {
	case in.ClearBindPassword:
		d.BindPassword = ""
	case d.BindPassword == "":
		d.BindPassword = s.Directory().BindPassword
	}
	b, _ := json.Marshal(d)
	return s.Store.SetSetting(settingDirectory, string(b))
}

// ResetDirectory drops the administrator's settings; the environment applies again.
func (s *Service) ResetDirectory() error { return s.Store.SetSetting(settingDirectory, "") }

// DirectoryStored says whether the administrator's settings are the ones in force.
func (s *Service) DirectoryStored() bool {
	v, ok := s.Store.Setting(settingDirectory)
	return ok && strings.TrimSpace(v) != ""
}

// TestDirectory binds with the given settings (nil = the ones in force). With a user it signs that user in
// (not stored); without one it checks the connection and the service account.
func (s *Service) TestDirectory(in *DirectoryInput, username, password string) (bool, string) {
	d := s.Directory()
	if in != nil {
		d = in.Directory
		d.Mode = "ldap"
		if err := ValidateDirectory(d); err != nil {
			return false, err.Error()
		}
		switch {
		case in.ClearBindPassword:
			d.BindPassword = ""
		case d.BindPassword == "":
			d.BindPassword = s.Directory().BindPassword
		}
	}
	if strings.TrimSpace(username) == "" {
		if err := LDAPCheckService(s.Dial, d); err != nil {
			return false, "LDAP error: " + err.Error()
		}
		if d.BindDN != "" {
			return true, "Connected and bound as the service account."
		}
		return true, "Connected (no service account set; searches are anonymous)."
	}
	if password == "" {
		return false, "Type a test user and password."
	}
	u, err := LDAPAuthenticate(s.Dial, d, strings.TrimSpace(username), password)
	switch {
	case err != nil:
		return false, "LDAP error: " + err.Error()
	case u == nil:
		return false, "Connected, but that user and password did not bind."
	}
	msg := fmt.Sprintf("Signed in as %s; groups: %s.", u.Display, orNone(shortGroups(u.Groups)))
	if len(d.AllowedGroups) > 0 && !AnyGroup(u.Groups, d.AllowedGroups) {
		return false, msg + " But the user is in none of the allowed groups."
	}
	if u.Admin {
		msg += " Administrator."
	}
	return true, msg
}

func shortGroups(g []string) string {
	var out []string
	for _, x := range g {
		if !strings.Contains(x, "=") {
			out = append(out, x)
		}
	}
	return strings.Join(out, ", ")
}

func orNone(s string) string {
	if s == "" {
		return "none"
	}
	return s
}

// ---- mock LDAP users --------------------------------------------------------------------

// SeedMock creates the mock directory's users: HUB_MOCK_USERS are added when missing on every start; the
// defaults (user1, user2) only once, so an administrator who deletes them keeps them deleted.
func (s *Service) SeedMock() error {
	users := s.Cfg.MockLDAPUsers
	if len(users) == 0 {
		if _, done := s.Store.Setting(settingMockSeed); done {
			return nil
		}
		users = []config.MockLDAPUser{{Name: "user1", Password: "user1pass"}, {Name: "user2", Password: "user2pass"}}
	}
	for _, u := range users {
		h, err := HashPassword(u.Password)
		if err != nil {
			return err
		}
		if _, err := s.Store.InsertUser(store.User{Name: u.Name, Hash: h, Admin: u.Admin, Source: SourceMock, Groups: u.Groups}); err != nil {
			return err
		}
	}
	if s.Cfg.MockLDAPUsers == nil {
		log.Printf("auth: mock LDAP directory enabled: users user1/user1pass and user2/user2pass (development only; manage them in /admin/users)")
	}
	return s.Store.SetSetting(settingMockSeed, "1")
}

// ---- admin view -------------------------------------------------------------------------

type adminOIDC struct {
	ID              string `json:"id"`
	Name            string `json:"name"`
	Issuer          string `json:"issuer"`
	ClientID        string `json:"clientId"`
	ClientSecretSet bool   `json:"clientSecretSet"`
	Scopes          string `json:"scopes"`
	AdminGroup      string `json:"adminGroup"`
	GroupsClaim     string `json:"groupsClaim"`
	UsernameClaim   string `json:"usernameClaim"`
	LoginURL        string `json:"loginUrl"`
	RedirectURI     string `json:"redirectUri"`
}

// AdminView is the GET /api/admin/directory body.
func (s *Service) AdminView(r *http.Request) map[string]any {
	d := s.Directory()
	src := "env"
	if s.DirectoryStored() {
		src = "admin"
	}
	base := s.ExternalBase(r)
	adfs := map[string]any{"enabled": s.adfsEnabled()}
	if s.adfsEnabled() {
		adfs["name"] = s.adfsName()
		adfs["redirectUri"] = base + "/auth/adfs/callback"
		adfs["loginUrl"] = "auth/adfs/login"
	}
	oidc := []adminOIDC{}
	for _, p := range s.Cfg.OIDCProviders {
		oidc = append(oidc, adminOIDC{ID: p.ID, Name: p.Name, Issuer: p.Issuer, ClientID: p.ClientID, ClientSecretSet: p.ClientSecret != "",
			Scopes: p.Scopes, AdminGroup: p.AdminGroup, GroupsClaim: p.GroupsClaim, UsernameClaim: p.UsernameClaim,
			LoginURL: "auth/oidc/" + p.ID + "/login", RedirectURI: base + "/auth/oidc/" + p.ID + "/callback"})
	}
	return map[string]any{"mode": d.Mode, "name": dirName(d.Mode), "source": src, "settings": viewOf(d), "adfs": adfs, "oidc": oidc}
}

// ---- sign-in ----------------------------------------------------------------------------

var errDirectoryUnavailable = errors.New("directory unavailable")

// directoryLogin signs in through the LDAP directory (mode ldap). (nil, nil) = wrong name or password or not
// allowed; an error = the directory could not be asked.
func (s *Service) directoryLogin(username, password string) (*DirUser, error) {
	d := s.Directory()
	u, err := LDAPAuthenticate(s.Dial, d, username, password)
	if err != nil {
		log.Printf("auth: ldap sign-in for %q failed: %v", username, err)
		return nil, errDirectoryUnavailable
	}
	if u == nil {
		return nil, nil
	}
	if len(d.AllowedGroups) > 0 && !AnyGroup(u.Groups, d.AllowedGroups) {
		log.Printf("auth: %s is not in an allowed group", username)
		return nil, nil
	}
	return u, nil
}

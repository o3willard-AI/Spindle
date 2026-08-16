# ── linux-baseline: Chaos drift detection controls (types 1-3) ──────────────
#
# These controls map directly to chaos types injectable by the chaos engine.
# Each control fails when its corresponding drift type is applied, and
# passes after `cinc-client --local-mode --override-runlist 'recipe[base]'`
# re-converges.

# packages-1.0 — Type 1: package-purge
# Chaos removes: htop, vim, tmux, curl
control 'packages-1.0' do
  impact 0.7
  title 'Base managed packages must be installed'
  desc 'htop, vim, tmux, and curl are installed by base::default and must be present.'

  %w[htop vim tmux curl].each do |pkg|
    describe package(pkg) do
      it { should be_installed }
    end
  end
end

# user-1.0 — Type 2: user-removal
# Chaos deletes: deploy user
control 'user-1.0' do
  impact 0.8
  title 'Deploy user must exist'
  desc 'The deploy user is created by base::default for application deployment.'

  describe etc_group.where(group_name: 'deploy') do
    it { should exist }
  end

  describe etc_passwd.where(user: 'deploy') do
    it { should exist }
  end

  describe file('/home/deploy') do
    it { should exist }
    it { should be_directory }
  end
end

# motd-1.0 — Type 3: motd-corrupt
# Chaos overwrites: /etc/motd
control 'motd-1.0' do
  impact 0.5
  title 'MOTD must contain managed content'
  desc 'The /etc/motd file is managed by base::default and must contain "CINC".'

  describe file('/etc/motd') do
    it { should exist }
    its('owner') { should eq 'root' }
    its('group') { should eq 'root' }
    its('mode') { should cmp '0644' }
    its('content') { should include 'CINC' }
    its('content') { should include 'managed by CINC' }
  end
end

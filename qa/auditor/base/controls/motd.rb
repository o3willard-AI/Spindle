# motd-1.0 — MOTD must contain managed content
control 'motd-1.0' do
  impact 0.5
  title 'MOTD must contain managed content'
  desc 'The /etc/motd file is managed by recipe[base] and must contain the node hostname and a managed marker.'

  motd = file('/etc/motd')
  describe motd do
    it { should exist }
    its('owner') { should eq 'root' }
    its('group') { should eq 'root' }
    its('mode') { should cmp '0644' }
  end

  describe command('hostname') do
    its('stdout') { should_not be_empty }
  end

  describe motd.content do
    it { should include 'CINC' }
  end
end

# user-1.0 — Deploy user must exist
control 'user-1.0' do
  impact 0.8
  title 'Deploy user must exist with correct attributes'
  desc 'The deploy user is created by recipe[base] for application deployment operations.'

  describe etc_group.where(group_name: 'deploy') do
    it { should exist }
  end

  describe etc_passwd.where(user: 'deploy') do
    it { should exist }
    its('shell') { should cmp '/bin/bash' }
    its('gid') { should eq etc_group.where(group_name: 'deploy').gids[0] }
  end
end

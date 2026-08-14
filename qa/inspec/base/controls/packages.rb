# packages-1.0 — Base system packages must be installed
control 'packages-1.0' do
  impact 0.7
  title 'Base system packages must be installed'
  desc 'htop, vim, tmux, and curl must be present — they are installed by recipe[base].'

  %w[htop vim tmux curl].each do |pkg|
    describe package(pkg) do
      it { should be_installed }
    end
  end
end

from lottery import *
import os
import numpy
from strategy import LinearStrategy

os.system("rm f.hist; rm leads.hist")

RUNNING_TIME = int(input("running time:"))

NODES=100

if __name__ == "__main__":
    darkies = []
    egalitarian = ERC20DRK/NODES
    darkies = []
    for id in range(int(NODES)):
      darkie = Darkie(random.gauss(egalitarian, egalitarian*0.1), strategy=random_strategy(EPOCH_LENGTH), apy_window=EPOCH_LENGTH)
      darkies += [darkie]

    #TODO try rpid with 0mint
    #darkies += [Darkie(0, strategy=LinearStrategy(EPOCH_LENGTH)) for _ in range(NODES)]
    airdrop = ERC20DRK
    effective_airdrop  = 0
    for darkie in darkies:
        effective_airdrop+=darkie.stake
    print("network airdrop: {}, staked token: {}/{}% on {} nodes".format(airdrop, effective_airdrop, effective_airdrop/airdrop*100, len(darkies)))
    dt = DarkfiTable(airdrop, RUNNING_TIME, CONTROLLER_TYPE_DISCRETE, kp=-0.010399999999938556, ki=-0.0365999996461878, kd=0.03840000000000491,  r_kp=-0.42, r_ki=2.71, r_kd=-0.239)
    for darkie in darkies:
        dt.add_darkie(darkie)
    acc, avg_apy, avg_reward, stake_ratio = dt.background_with_apy(rand_running_time=False)
    sum_zero_stake = sum([darkie.stake for darkie in darkies[NODES:]])
    print('acc: {}, avg(apy): {}, avg(reward): {}, stake_ratio: {}'.format(acc, avg_apy, avg_reward, stake_ratio))
    print('total stake of 0mint: {}, ration: {}'.format(sum_zero_stake, sum_zero_stake/ERC20DRK))
    dt.write()
